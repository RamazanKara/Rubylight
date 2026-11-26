/**
 * Host preflight, two-step codec/HDR/SharpColor format negotiation, jumbo/link sizing,
 * and native stream bridge start for {@link NvConnection}, calling {@link NightjarSessionJni}.
 */
package com.limelight.nvstream;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.IpPrefix;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.RouteInfo;
import android.os.Build;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.audio.Surround714;
import com.limelight.binding.video.PyroWaveDecoderRenderer;
import com.limelight.computers.HostNotices;
import com.limelight.computers.HostPackedModes;
import com.limelight.computers.InternetPlay;
import com.limelight.nvstream.av.audio.AudioRenderer;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.FecChoice;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarSessionJni;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.NetworkGrace;
import com.limelight.utils.PyroWavePaceStore;
import com.limelight.utils.RecentStreams;
import com.limelight.utils.WiredLink;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;
import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;

import javax.net.ssl.SSLException;

final class NvConnectionPreflight {
    private NvConnectionPreflight() {}

    interface AddressLookup {
        InetAddress[] resolveAll(String host) throws java.net.UnknownHostException;
        boolean probeReachable(InetAddress addr, int port, int timeoutMs);
    }

    private static final AddressLookup DEFAULT_ADDRESS_LOOKUP = new AddressLookup() {
        @Override
        public InetAddress[] resolveAll(String host) throws java.net.UnknownHostException {
            return InetAddress.getAllByName(host);
        }

        @Override
        public boolean probeReachable(InetAddress addr, int port, int timeoutMs) {
            try (Socket s = new Socket()) {
                s.setSoLinger(true, 0);
                s.connect(new InetSocketAddress(addr, port), timeoutMs);
                return true;
            } catch (IOException e) {
                e.printStackTrace();
                return false;
            }
        }
    };

    static InetAddress resolveServerAddress(ConnectionContext context) throws IOException {
        return resolveServerAddress(context, DEFAULT_ADDRESS_LOOKUP);
    }

    static InetAddress resolveServerAddress(ConnectionContext context, AddressLookup lookup) throws IOException {
        InetAddress[] addrs = lookup.resolveAll(context.serverAddress.address);
        if (addrs.length == 1) {
            return addrs[0];
        }
        for (InetAddress addr : addrs) {
            if (lookup.probeReachable(addr, context.serverAddress.port, 1000)) {
                return addr;
            }
        }
        if (addrs.length > 0) return addrs[0];
        throw new IOException("No addresses found for " + context.serverAddress);
    }

    private static int detectServerConnectionType(Context appContext, ConnectionContext context) {
        if (context.serverAddress != null && InternetPlay.isTunnelAddress(context.serverAddress.address)) {
            return StreamConfiguration.STREAM_CFG_REMOTE;
        }
        ConnectivityManager connMgr =
                (ConnectivityManager) appContext.getSystemService(Context.CONNECTIVITY_SERVICE);
        Network activeNetwork = connMgr.getActiveNetwork();
        if (activeNetwork != null) {
            NetworkCapabilities netCaps = connMgr.getNetworkCapabilities(activeNetwork);
            if (netCaps != null && (netCaps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)
                    || !netCaps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
                    || netCaps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR))) {
                return StreamConfiguration.STREAM_CFG_REMOTE;
            }
            LinkProperties linkProperties = connMgr.getLinkProperties(activeNetwork);
            if (linkProperties != null) {
                InetAddress serverAddress;
                try {
                    serverAddress = resolveServerAddress(context);
                } catch (IOException e) {
                    e.printStackTrace();
                    return StreamConfiguration.STREAM_CFG_AUTO;
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    IpPrefix nat64Prefix = linkProperties.getNat64Prefix();
                    if (nat64Prefix != null && nat64Prefix.contains(serverAddress)) {
                        return StreamConfiguration.STREAM_CFG_REMOTE;
                    }
                }
                for (RouteInfo route : linkProperties.getRoutes()) {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
                            && route.getType() != RouteInfo.RTN_UNICAST) {
                        continue;
                    }
                    if (route.matches(serverAddress)) {
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                            if (!route.hasGateway()) return StreamConfiguration.STREAM_CFG_LOCAL;
                        } else {
                            InetAddress gw = route.getGateway();
                            if (gw == null || gw.isAnyLocalAddress()) return StreamConfiguration.STREAM_CFG_LOCAL;
                        }
                    }
                }
            }
        }
        return StreamConfiguration.STREAM_CFG_AUTO;
    }

    private static void applyNegotiationNotices(Context appContext, ConnectionContext context,
            ComputerDetails details, VideoDecoderRenderer renderer, JSONArray notices, String packedHdrDrop) {
        for (int i = 0; i < notices.length(); i++) {
            String tag = notices.optString(i, "");
            switch (tag) {
                case "sdr10_given_way_to_packed":
                    LimeLog.warning("10-bit SDR not used: packed full chroma takes precedence");
                    if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.SDR10_UNAVAILABLE)) {
                        context.connListener.displayTransientMessage(appContext.getString(
                                R.string.stream_sdr10_unavailable, appContext.getString(R.string.stream_sdr10_reason_packed)));
                    }
                    break;
                case "packed_unavailable_from_host":
                case "packed_unavailable_from_device":
                    if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.PACKED_UNAVAILABLE)) {
                        context.connListener.displayTransientMessage(appContext.getString(
                                "packed_unavailable_from_host".equals(tag)
                                        ? R.string.sharp_color_unavailable_pc : R.string.sharp_color_unavailable_device));
                    }
                    break;
                case "packed_available":
                    HostNotices.reset(appContext, details.uuid, HostNotices.Notice.PACKED_UNAVAILABLE);
                    break;
                case "pyrowave_hdr_lost":
                    int reason = renderer.pyroWaveHdrFailureReason();
                    LimeLog.warning("PyroWave HDR unavailable, streaming "
                            + Integer.toHexString(context.negotiatedVideoFormats)
                            + (reason != 0 ? ": " + appContext.getString(reason) : ""));
                    if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.PYROWAVE_HDR_UNAVAILABLE)) {
                        context.connListener.displayTransientMessage(appContext.getString(
                                R.string.startup_pyrowave_hdr_unavailable,
                                appContext.getString(reason != 0 ? reason : R.string.pyrowave_check_failed)));
                    }
                    break;
                case "pyrowave_hdr_accepted":
                    HostNotices.reset(appContext, details.uuid, HostNotices.Notice.PYROWAVE_HDR_UNAVAILABLE);
                    break;
                case "sdr10_given_way_to_yuv444":
                    if (packedHdrDrop != null) break;
                    LimeLog.warning("10-bit SDR not used: native 4:4:4 takes precedence");
                    if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.SDR10_UNAVAILABLE)) {
                        context.connListener.displayTransientMessage(appContext.getString(
                                R.string.stream_sdr10_unavailable, appContext.getString(R.string.stream_sdr10_reason_yuv444)));
                    }
                    break;
                case "sdr10_not_accepted":
                    if (packedHdrDrop != null) break;
                    HostNotices.reset(appContext, details.uuid, HostNotices.Notice.SDR10_UNAVAILABLE);
                    LimeLog.warning("10-bit SDR offered, but this PC streams 8-bit");
                    if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.SDR10_NOT_ACCEPTED)) {
                        context.connListener.displayTransientMessage(appContext.getString(R.string.startup_sdr10_not_accepted));
                    }
                    break;
                case "sdr10_accepted":
                    if (packedHdrDrop != null) break;
                    HostNotices.reset(appContext, details.uuid, HostNotices.Notice.SDR10_UNAVAILABLE);
                    HostNotices.reset(appContext, details.uuid, HostNotices.Notice.SDR10_NOT_ACCEPTED);
                    break;
                case "hdr_unavailable":
                    if (packedHdrDrop != null) break;
                    if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.HDR_UNAVAILABLE)) {
                        context.connListener.displayTransientMessage(appContext.getString(R.string.startup_hdr_unavailable));
                    }
                    break;
                case "hdr_accepted":
                    if (packedHdrDrop != null) break;
                    HostNotices.reset(appContext, details.uuid, HostNotices.Notice.HDR_UNAVAILABLE);
                    if (!context.streamConfig.isPacked444()) {
                        HostNotices.reset(appContext, details.uuid, HostNotices.Notice.PACKED_HDR_UNAVAILABLE);
                    }
                    break;
                default:
                    break;
            }
        }
    }

    private static void negotiateMediaFormats(
            NvConnection conn, ComputerDetails details, VideoDecoderRenderer renderer) {
        Context appContext = conn.appContext;
        ConnectionContext context = conn.context;
        boolean sharpColor = context.streamConfig.isSharpColor();
        boolean sdr10 = context.streamConfig.isSdr10Bit();
        boolean packedEngine = context.streamConfig.isPacked444();
        int hostPacked = -1;
        boolean refusedBefore = false;
        if (packedEngine) {
            if (sharpColor) {
                HostPackedModes.renew(appContext, details.uuid,
                        HostPackedModes.stamp(BuildConfig.VERSION_NAME, details.caps.hostVersion()));
                refusedBefore = HostPackedModes.refused(appContext, details.uuid);
            }
            hostPacked = NvConnection.hostPackedModesBeforeLaunch(appContext, details);
            renderer.setHostPacked444Modes(hostPacked);
        }
        renderer.setStreamFrameRateMilliHz(NvHTTP.getStreamFrameRateMilliHz(context));
        renderer.setHostCodecModeSupport(context.serverCodecModeSupport);

        NightjarSessionJni.NegotiationCallback cb = new NightjarSessionJni.NegotiationCallback() {
            @Override
            public int preparePackedModes(int probeFormats) {
                return renderer.preparePacked444Modes(probeFormats, context.negotiatedWidth,
                        context.negotiatedHeight, context.streamConfig.getRefreshRate());
            }

            @Override
            public int prepareVideoFormats(int commonFormats, int hostScm) {
                return renderer.prepareVideoFormats(commonFormats, context.negotiatedWidth,
                        context.negotiatedHeight, context.streamConfig.getRefreshRate());
            }

            @Override
            public long pickHdrFormat(int hostHdrFormats, int negotiatedFormats, boolean packed444, boolean frameWarp) {
                int hdrFmt = renderer.pickHdrFormat(hostHdrFormats, negotiatedFormats, packed444, frameWarp);
                int adj = renderer.videoFormatsForHdrFormat(hdrFmt, negotiatedFormats);
                return ((long) hdrFmt << 32) | (adj & 0xffffffffL);
            }

            @Override
            public int videoCapabilities(int negotiatedFormats, int hdrFormat) {
                return renderer.getCapabilities(negotiatedFormats, hdrFormat);
            }
        };

        String reply = NightjarSessionJni.negotiateTwoStep(
                context.streamConfig.getSupportedVideoFormats(), context.serverCodecModeSupport,
                details.caps.hdrFormats(), details.caps.has(HostCapabilities.Feature.FRAME_WARP),
                sharpColor, sdr10, context.streamConfig.isPyroWaveChosen(), packedEngine,
                hostPacked, refusedBefore, cb);
        try {
            JSONObject out = new JSONObject(reply);
            int offeredFormats = out.getInt("offered_formats");
            context.negotiatedVideoFormats = out.getInt("negotiated_formats");
            context.negotiated10Bit = out.getBoolean("negotiated_10bit");
            context.negotiatedHdr = out.getBoolean("negotiated_hdr");
            context.hdrFormat = out.getInt("hdr_format");
            context.videoCapabilities = out.getInt("video_capabilities");
            boolean packed444 = out.getBoolean("packed_444");
            boolean native444 = out.getBoolean("native_444");
            boolean keepSdr10 = out.getBoolean("keep_sdr10");
            if (sharpColor) {
                LimeLog.info("Sharper colored text: " + out.optString("sharp_pick_text", "neither")
                        + " (hostScm=0x" + Integer.toHexString(context.serverCodecModeSupport)
                        + ", decodable=0x" + Integer.toHexString(out.optInt("sharp_decodable", 0)) + ")");
                renderer.setPacked444(packed444);
                renderer.setNative444(native444);
                context.streamConfig.applySharpColor(packed444, offeredFormats, keepSdr10);
            }
            String packedHdrDrop = renderer.packedHdrDropReason();
            conn.packedHdrDropAtLaunch = packedHdrDrop;
            if (packedHdrDrop != null) {
                NvConnection.announcePackedHdrDrop(appContext, details.uuid, context.connListener, packedHdrDrop);
            }
            JSONArray notices = out.optJSONArray("notices");
            if (notices != null) {
                applyNegotiationNotices(appContext, context, details, renderer, notices, packedHdrDrop);
            }
        } catch (JSONException e) {
            LimeLog.severe("Malformed media negotiation reply: " + e.getMessage());
            throw new IllegalStateException("Malformed media negotiation reply", e);
        }
    }

    static boolean startApp(NvConnection conn, VideoDecoderRenderer renderer)
            throws XmlPullParserException, IOException {
        Context appContext = conn.appContext;
        ConnectionContext context = conn.context;
        NvHTTP h = new NvHTTP(context.serverAddress, context.httpsPort, conn.uniqueId,
                context.serverCert, conn.cryptoProvider);
        synchronized (conn) {
            conn.http = h;
            if (conn.stopRequested) h.cancelPendingRequests();
        }
        String serverInfo = h.getServerInfo(true);
        context.serverAppVersion = h.getServerVersion(serverInfo);
        if (context.serverAppVersion == null) {
            context.connListener.displayMessage(appContext.getString(R.string.startup_invalid_host));
            return false;
        }
        ComputerDetails details = h.getComputerDetails(serverInfo);
        conn.hostDetails = details;
        context.isNvidiaServerSoftware = details.nvidiaServer;
        context.hostCaps = details.caps;
        LimeLog.info("Host: " + details.caps);
        context.streamConfig.getApp().setHostCapabilities(details.caps);
        context.streamConfig.setHostAudioConfiguration(
                Surround714.forHost(context.streamConfig.getAudioConfiguration(), details.caps));
        if (conn.canSendControllerInput()
                && details.caps.has(HostCapabilities.Feature.CONTROLLER_PREPARATION)
                && conn.controllerPreparation != null) {
            context.preparedControllers = conn.controllerPreparation.get();
        }
        context.hostDisplay = context.streamConfig.getHostDisplay() != null
                ? context.streamConfig.getHostDisplay() : HostDisplayChoice.load(appContext, details.uuid);
        NvApp.Role role = context.streamConfig.getApp().getRole();
        boolean virtualDisplay = details.caps.has(HostCapabilities.Feature.VIRTUAL_DISPLAY);
        if (virtualDisplay) {
            HostNotices.reset(appContext, details.uuid, HostNotices.Notice.VIRTUAL_DISPLAY_UNAVAILABLE);
        } else if (NvHTTP.wantsVirtualDisplay(context, role)
                && HostNotices.claim(appContext, details.uuid, HostNotices.Notice.VIRTUAL_DISPLAY_UNAVAILABLE)) {
            context.connListener.displayTransientMessage(appContext.getString(R.string.startup_virtual_display_unavailable));
        }
        if (h.getPairState(serverInfo) != PairingManager.PairState.PAIRED) {
            context.connListener.displayMessage(appContext.getString(R.string.startup_not_paired));
            return false;
        }
        NvApp app = context.streamConfig.getApp();
        NvApp currentApp = null;
        boolean join = context.streamConfig.isJoinRunning();
        if (!join && context.streamConfig.isResumeOnly()
                && (app.getControl() != NvApp.Control.NONE
                        || !app.matchesRunningApp(details.runningGameId, details.runningGameUuid))) {
            String movedTo = NvConnectionActions.readStreamMovedTo(conn, false);
            if (movedTo != null) {
                context.connListener.displayMessage(NvConnection.movedMessage(appContext, movedTo));
                context.connListener.streamMoved(movedTo);
                return false;
            }
            if (details.runningGameId == 0 && context.streamConfig.isIdleSessionEndAllowed()
                    && NvConnectionActions.hasEndedSession(conn, h)) {
                throw new NvConnection.SessionEndedException();
            }
            context.connListener.displayMessage(appContext.getString(R.string.stream_recovery_session_changed));
            return false;
        }
        if (!join && !app.getAppUuid().isEmpty()) {
            if (details.runningGameId != 0 && app.matchesRunningApp(details.runningGameId, details.runningGameUuid)) {
                app.setAppId(details.runningGameId);
            } else {
                currentApp = details.hasPermission(ComputerDetails.PERMISSION_LIST) ? h.getAppByUuid(app.getAppUuid()) : null;
                if (currentApp != null) {
                    app.setAppId(currentApp.getAppId());
                    app.setMonitorResume(currentApp.getRole() == NvApp.Role.REMOTE_MONITOR);
                } else if (details.caps.has(HostCapabilities.Feature.LAUNCH_BY_UUID)) {
                    app.setAppId(0);
                }
            }
        }
        if (!app.isInitialized()) {
            currentApp = h.getAppByName(app.getAppName());
            if (currentApp == null) {
                context.connListener.displayMessage(appContext.getString(R.string.startup_app_missing));
                return false;
            }
            app.setAppId(currentApp.getAppId());
            app.setAppUuid(currentApp.getAppUuid());
            app.setMonitorResume(currentApp.getRole() == NvApp.Role.REMOTE_MONITOR);
        }
        if (app.getControl() == NvApp.Control.RESUME) {
            if (currentApp == null) {
                currentApp = app.getAppUuid().isEmpty() ? h.getAppById(app.getAppId()) : h.getAppByUuid(app.getAppUuid());
            }
            if (currentApp == null) {
                context.connListener.displayMessage(appContext.getString(R.string.startup_resume_missing));
                return false;
            }
            app.setMonitorResume(currentApp.getRole() == NvApp.Role.REMOTE_MONITOR);
        }

        context.serverCodecModeSupport =
                details.caps.trustedCodecModeSupport((int) h.getServerCodecModeSupport(serverInfo));
        if (app.isSmallStreamInputOnly()) {
            context.negotiatedWidth = context.streamConfig.getWidth();
            context.negotiatedHeight = context.streamConfig.getHeight();
            context.negotiatedVideoFormats = MoonBridge.VIDEO_FORMAT_H264;
        } else if (app.getRole() == NvApp.Role.INPUT_ONLY || app.isControlAction()) {
            context.negotiatedWidth = 1280;
            context.negotiatedHeight = 720;
            context.negotiatedVideoFormats = MoonBridge.VIDEO_FORMAT_H264;
        } else {
            if ((context.streamConfig.getWidth() > 4096 || context.streamConfig.getHeight() > 4096)
                    && (h.getServerCodecModeSupport(serverInfo) & 0x200) == 0 && context.isNvidiaServerSoftware) {
                context.connListener.displayMessage(appContext.getString(R.string.startup_host_resolution_unsupported));
                return false;
            } else if ((context.streamConfig.getWidth() > 4096 || context.streamConfig.getHeight() > 4096)
                    && (context.streamConfig.getSupportedVideoFormats() & ~MoonBridge.VIDEO_FORMAT_MASK_H264) == 0) {
                context.connListener.displayMessage(appContext.getString(R.string.startup_device_resolution_unsupported));
                return false;
            } else if (context.streamConfig.getHeight() >= 2160 && !h.supports4K(serverInfo)) {
                if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.HOST_4K_UNAVAILABLE)) {
                    context.connListener.displayTransientMessage(appContext.getString(R.string.startup_host_update_4k));
                }
                context.negotiatedWidth = 1920;
                context.negotiatedHeight = 1080;
            } else {
                context.negotiatedWidth = context.streamConfig.getWidth();
                context.negotiatedHeight = context.streamConfig.getHeight();
            }
            negotiateMediaFormats(conn, details, renderer);
        }

        context.linkHints = WiredLink.hintsToward(appContext, context.serverAddress.address);
        RecentStreams.noteLink(context.linkHints);
        context.startBitrateKbps = NightjarSessionJni.pyroWaveStartKbps(
                context.negotiatedVideoFormats, context.streamConfig.getBitrate(),
                context.streamConfig.isAutomaticBitrate(), context.negotiatedWidth,
                context.negotiatedHeight, context.streamConfig.getRefreshRate(), context.linkHints.mbps,
                context.streamConfig.getBitrateLimitKbps());
        if ((context.negotiatedVideoFormats & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            int[] notice = NightjarSessionJni.pyroWaveLinkNotice(
                    context.negotiatedVideoFormats, context.negotiatedWidth, context.negotiatedHeight,
                    context.streamConfig.getRefreshRate(), context.linkHints.mbps,
                    context.streamConfig.getBitrate(), context.streamConfig.isAutomaticBitrate());
            StringBuilder pyrowaveNotices = new StringBuilder();
            if (notice != null && notice.length >= 2) {
                pyrowaveNotices.append(appContext.getString(R.string.pyrowave_link_notice,
                        context.negotiatedWidth, context.negotiatedHeight,
                        context.streamConfig.getRefreshRate(), (notice[0] + 999) / 1000, (notice[1] + 999) / 1000));
            }
            int minimum = PyroWaveBitrateFloor.minimumKbps(
                    context.negotiatedWidth, context.negotiatedHeight, context.streamConfig.getRefreshRate());
            if (context.startBitrateKbps < minimum) {
                if (pyrowaveNotices.length() > 0) pyrowaveNotices.append('\n');
                pyrowaveNotices.append(appContext.getString(R.string.summary_bitrate_pyrowave_below_minimum,
                        (minimum + 999) / 1000, context.negotiatedWidth, context.negotiatedHeight,
                        context.streamConfig.getRefreshRate()));
            }
            if (pyrowaveNotices.length() > 0) context.connListener.displayTransientMessage(pyrowaveNotices.toString());
        }

        int rawRemote = context.streamConfig.getRemote();
        context.negotiatedRemoteStreaming = rawRemote == StreamConfiguration.STREAM_CFG_AUTO
                ? detectServerConnectionType(appContext, context) : rawRemote;
        int locationOrd = rawRemote == StreamConfiguration.STREAM_CFG_LOCAL ? 1
                : rawRemote == StreamConfiguration.STREAM_CFG_REMOTE ? 2 : 0;
        boolean vpnOrTunnel = rawRemote == StreamConfiguration.STREAM_CFG_AUTO
                && context.negotiatedRemoteStreaming == StreamConfiguration.STREAM_CFG_REMOTE;
        boolean jumboFrames = PreferenceConfiguration.jumboFrames(appContext);
        NightjarSessionJni.LinkProbe paceProbe = new NightjarSessionJni.LinkProbe() {
            @Override public String probeLinkMbps() { return "{\"error\":{\"kind\":\"cancelled\",\"message\":\"\"}}"; }
            @Override public String probePace(int kbps, int size, int fps, int pace) {
                try {
                    PyroWavePace.Result res = h.probePyroWaveUdp(kbps, size, fps, pace);
                    return new JSONObject().put("ok", new JSONObject()
                            .put("expected", res.expected).put("sent", res.sent).put("received", res.received)
                            .put("sender_ms", res.senderMs).put("loss", res.lossPercent)
                            .put("worst_window_loss", res.worstWindowLossPercent).put("frames", res.frames)
                            .put("damaged_frames", res.damagedFrames).put("excessive_loss_frames", res.excessiveLossFrames)
                            .put("missing_tail_frames", res.missingTailFrames).put("worst_frame_loss", res.worstFrameLossPercent)).toString();
                } catch (Throwable t) {
                    return NvConnectionLaunch.errorEnvelope(t);
                }
            }
        };
        // A resume keeps its packet size only on the network that negotiated it: another path may drop
        // jumbo packets whole.
        Network network = RoamingNetworkWatch.defaultNetwork(appContext);
        context.packetNetwork = network != null ? network.getNetworkHandle() : 0;
        boolean keepsPacketSize = context.streamConfig.isResumeOnly() && context.streamConfig.getResumePacketSize() >= 0
                && context.packetNetwork != 0
                && context.packetNetwork == context.streamConfig.getResumePacketNetwork();
        int[] pktRes = NightjarSessionJni.resolvePacketSizePreflight(
                locationOrd, context.negotiatedRemoteStreaming != StreamConfiguration.STREAM_CFG_REMOTE,
                vpnOrTunnel,
                context.hostCaps.maxVideoPacket(), details.supportsPyroWavePaceCalibration(),
                context.streamConfig.getMaxPacketSize(), jumboFrames, keepsPacketSize,
                context.streamConfig.getResumePacketSize(), paceProbe);
        context.negotiatedPacketSize = pktRes != null && pktRes.length >= 1
                ? pktRes[0] : context.streamConfig.getMaxPacketSize();
        context.jumboFallback = pktRes != null && pktRes.length >= 2 && pktRes[1] != 0;

        if (!details.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH)) {
            context.connListener.displayMessage(appContext.getString(R.string.startup_permission_denied));
            return false;
        }
        return conn.launchOrResume(h, context, details, app, serverInfo);
    }

    static void runConnection(NvConnection conn, AudioRenderer audioRenderer,
            VideoDecoderRenderer videoDecoderRenderer, NvConnectionListener connectionListener) {
        ConnectionContext context = conn.context;
        Context appContext = conn.appContext;
        context.connListener = connectionListener;
        String appName = context.streamConfig.getApp().getAppName();
        boolean acquired = false;
        boolean started = false;
        try {
            if (conn.stopRequested) return;
            context.videoCapabilities = videoDecoderRenderer.getCapabilities();
            context.connListener.stageStarting(appName);
            if (!NvConnection.acquireConnection()) {
                if (!conn.stopRequested) {
                    context.connListener.displayMessage(appContext.getString(R.string.stream_stop_timeout));
                    context.connListener.stageFailed(appName, 0, 0);
                }
                return;
            }
            acquired = true;
            if (!startApp(conn, videoDecoderRenderer)) {
                if (!conn.stopRequested && !context.launchActionCompleted) {
                    context.connListener.stageFailed(appName, 0, 0);
                }
                return;
            }
            if (conn.stopRequested) return;
            context.connListener.stageComplete(appName);
            if (!context.launchActionCompleted) {
                boolean pyroWave = (context.negotiatedVideoFormats & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0;
                WiredLink.Hints linkHints = context.linkHints;
                int pyroWaveLinkMbps = linkHints.mbps;
                boolean wirelessRoute = linkHints.wireless;
                PreferenceConfiguration hintPrefs = PreferenceConfiguration.readPreferences(appContext);
                // Frame Warp on and a PC whose streaming app is older than this Nightjar's: the stream runs
                // without it and says why, as a renderer that cannot draw it does
                HostCapabilities.Availability frameWarp = conn.hostDetails.caps.availability(HostCapabilities.Feature.FRAME_WARP);
                boolean frameWarpHostTooOld = frameWarp.reason == HostCapabilities.Reason.UPDATE_HOST;
                if (hintPrefs.frameWarp && frameWarpHostTooOld) {
                    context.connListener.displayTransientMessage(appContext.getString(R.string.frame_warp_update_host));
                }
                boolean clientWireless = WiredLink.tellsHostWireless(
                        hintPrefs.tellHostWireless, conn.hostDetails.caps.family(), wirelessRoute);
                int sentLinkMbps = hintPrefs.tellHostLinkSpeed ? pyroWaveLinkMbps : 0;
                int pyroWavePaceMbps = pyroWave
                        ? PyroWavePaceStore.paceMbpsFor(appContext, conn.hostDetails.uuid, pyroWaveLinkMbps) : 0;
                ByteBuffer ib = ByteBuffer.allocate(16).order(ByteOrder.BIG_ENDIAN);
                ib.putInt(context.riKeyId);

                synchronized (MoonBridge.class) {
                    synchronized (conn) {
                        if (conn.stopRequested) return;
                        conn.nativeStarted = true;
                    }
                    MoonBridge.setupBridge(videoDecoderRenderer, audioRenderer, connectionListener, () -> conn.stopRequested);
                    conn.readMicrophoneSettings(hintPrefs);
                    if (conn.micEnabled && !conn.micPermitted) {
                        LimeLog.warning("Microphone: on in settings but the permission is not granted; not sending");
                    }
                    MoonBridge.setNetworkGracePeriodMs(NetworkGrace.nativeMilliseconds(hintPrefs.networkGraceSeconds));
                    MoonBridge.requestPacked444(context.streamConfig.isPacked444());
                    MoonBridge.requestHdrFormat(videoDecoderRenderer.hdrFormatRequest(context.hdrFormat));
                    MoonBridge.requestVideoFec((context.fecChoice != null ? context.fecChoice : FecChoice.HOST).summaryPercentage());
                    MoonBridge.requestPyroWaveEarly(PyroWaveDecoderRenderer.wantsEarlyRecords(
                            pyroWave, conn.hostDetails.caps.has(HostCapabilities.Feature.PYROWAVE_RECORDS),
                            PyroWaveDecoderRenderer.earlyRecordsAllowed()));
                    MoonBridge.requestCodecDirect(hintPrefs.codecDirect);
                    MoonBridge.requestReceiveDecodePriority(hintPrefs.receiveDecodePriority);
                    MoonBridge.requestReceiveCoalescing(hintPrefs.receiveCoalescing);
                    MoonBridge.setDecodeQueueBound(hintPrefs.decodeQueueFrames);
                    MoonBridge.requestRoaming(hintPrefs.clientRoaming, hintPrefs.roamingMetered);
                    MoonBridge.requestAudioPassthrough(NvConnectionAudio.audioPassthroughCodecs(
                            hintPrefs.audioPassthrough, hintPrefs.muteAudio,
                            context.streamConfig.getAudioConfiguration().channelCount));
                    MoonBridge.requestFrameWarp(hintPrefs.frameWarp, frameWarp.isSupported(), frameWarpHostTooOld);
                    Network streamNetwork = RoamingNetworkWatch.defaultNetwork(appContext);
                    int ret = MoonBridge.startConnection(
                            context.serverAddress.address, context.serverAppVersion, context.rtspSessionUrl,
                            context.serverCodecModeSupport, context.negotiatedWidth, context.negotiatedHeight,
                            context.streamConfig.getRefreshRate(), context.startBitrateKbps,
                            context.negotiatedPacketSize, context.negotiatedRemoteStreaming,
                            context.streamConfig.getAudioConfiguration().toInt(), context.negotiatedVideoFormats,
                            context.streamConfig.getClientRefreshRateX100(), context.riKey.getEncoded(), ib.array(),
                            context.videoCapabilities, context.streamConfig.getColorSpace(), context.streamConfig.getColorRange(),
                            hintPrefs.unbatchedInput, hintPrefs.networkPriority, conn.hostDetails.caps.nativeFeatures(),
                            context.streamConfig.getApp().getRole() == NvApp.Role.INPUT_ONLY
                                    && !context.streamConfig.getApp().isSmallStreamInputOnly(),
                            context.streamConfig.getApp().getRole() == NvApp.Role.REMOTE_MONITOR,
                            conn.micPermitted, hintPrefs.usbDualSenseHaptics, sentLinkMbps, pyroWavePaceMbps, clientWireless);
                    if (context.streamConfig.isPacked444()) {
                        NvConnection.rememberPackedModes(appContext, conn.hostDetails.uuid, ret,
                                ret == 0 ? MoonBridge.getHostPacked444Modes() : 0);
                    }
                    if (ret != 0) {
                        if (!conn.stopRequested && !MoonBridge.startFailureReported()) {
                            LimeLog.warning("The stream start failed (" + ret + ") without a failed stage");
                            context.connListener.stageFailed(appName, 0, ret);
                        }
                        return;
                    }
                    if (context.streamConfig.isPacked444()) {
                        HostNotices.reset(appContext, conn.hostDetails.uuid, HostNotices.Notice.PACKED_UNAVAILABLE);
                        NvConnection.noticePackedAfterStart(appContext, conn.hostDetails.uuid, context.connListener,
                                context.negotiatedHdr, conn.packedHdrDropAtLaunch, videoDecoderRenderer.packedHdrDropReason());
                    }
                    conn.nativeConnected = true;
        started = true;
                    if (context.jumboFallback) {
                        context.connListener.displayTransientMessage(appContext.getString(R.string.jumbo_frames_fallback));
                    }
                    conn.roamingWatch = RoamingNetworkWatch.start(appContext, streamNetwork, context.connListener,
                            () -> conn.networkMoved = true);
                    conn.startInputPolling(hintPrefs.unbatchedInput);
                    NvConnectionAudio.startMicrophone(conn, false);
                    started = true;
                }
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } catch (NvConnection.SessionEndedException e) {
            if (!conn.stopRequested) context.connListener.connectionTerminated(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION);
        } catch (NvHTTP.LaunchTimeoutException e) {
            if (!conn.stopRequested) {
                LimeLog.warning("The PC did not answer the start within " + e.seconds + " s");
                context.connListener.displayMessage(appContext.getString(R.string.startup_launch_timeout, e.seconds));
                context.connListener.stageFailed(appName, 0, 0);
            }
        } catch (HostHttpResponseException e) {
            if (!conn.stopRequested) {
                e.printStackTrace();
                context.connListener.displayMessage(NvConnectionLaunch.launchFailureMessage(appContext, e));
                context.connListener.stageFailed(appName, 0, e.getErrorCode());
            }
        } catch (XmlPullParserException | IOException | RuntimeException e) {
            if (!conn.stopRequested) {
                e.printStackTrace();
                context.connListener.displayMessage(appContext.getString(R.string.startup_connection_failed));
                int retryPorts = e instanceof IOException && !(e instanceof SSLException)
                        ? MoonBridge.ML_PORT_FLAG_TCP_47984 | MoonBridge.ML_PORT_FLAG_TCP_47989 : 0;
                context.connListener.stageFailed(appName, retryPorts, 0);
            }
        } finally {
            if (!started) {
                try {
                    synchronized (MoonBridge.class) {
                        synchronized (conn) {
                            if (conn.nativeStarted) {
                                conn.nativeStarted = false;
                                MoonBridge.cleanupBridge();
                            }
                        }
                        videoDecoderRenderer.cleanup();
                    }
                } finally {
                    if (acquired) NvConnection.releaseConnection();
                }
            }
        }
    }
}
