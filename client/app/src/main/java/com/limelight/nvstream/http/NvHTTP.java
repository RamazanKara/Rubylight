/**
 * Non-final HTTP/TLS host client facade delegating networking, pairing, XML/JSON parsing, launch,
 * probes, and clipboard operations to {@code nightjar_core::http::HostClient} via {@link NightjarHttpJni}.
 */
package com.limelight.nvstream.http;

import android.os.Build;

import com.limelight.BuildConfig;
import com.limelight.nvstream.ConnectionContext;
import com.limelight.nvstream.PyroWavePace;
import com.limelight.nvstream.jni.HandleReaper;
import com.limelight.nvstream.jni.Json;
import com.limelight.nvstream.jni.NightjarHttpJni;
import com.limelight.preferences.HostDisplayChoice;

import org.xmlpull.v1.XmlPullParserException;

import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.io.Reader;
import java.security.PrivateKey;
import java.security.cert.CertificateEncodingException;
import java.security.cert.X509Certificate;
import java.util.LinkedList;
import java.util.function.IntConsumer;

public class NvHTTP implements AutoCloseable {
    public static final int DEFAULT_HTTP_PORT = 47989;
    public static final int SHORT_CONNECTION_TIMEOUT = 3000;
    public static final int LONG_CONNECTION_TIMEOUT = 5000;
    public static final int READ_TIMEOUT = 7000;
    public static final int PYROWAVE_BANDWIDTH_PROBE_BYTES = 32 * 1024 * 1024;
    public static final String RATE_CHANGE_LAUNCH_PARAMETER = "rlRateChange";
    public static final String SECOND_SCREEN_PARAMETER = "rlSecondScreen";
    public static final int LINK_PROBE_BYTES = 4 * 1024 * 1024;
    public static final int LAUNCH_READ_TIMEOUT_SECONDS = 120;
    public static final String USER_AGENT = "Nightjar/" + BuildConfig.VERSION_NAME;
    static final int MAX_XML_RESPONSE_BYTES = 256 * 1024;
    static final int MAX_APP_LIST_BYTES = 16 * 1024 * 1024;

    static {
        NightjarHttpJni.setAppVersion(BuildConfig.VERSION_NAME);
    }

    @FunctionalInterface
    interface SnapshotParser {
        String parse(String serverInfoXml, int httpPort, boolean strict);
    }

    private static final class ParsedSnapshotCache {
        final String xml;
        final Json.Obj snapshot;

        ParsedSnapshotCache(String xml, Json.Obj snapshot) {
            this.xml = xml;
            this.snapshot = snapshot;
        }
    }

    private final long handle;
    private final HandleReaper.Token reaper;
    private final int httpPort;
    private X509Certificate serverCert;
    private volatile boolean serverInfoFromAnotherCertificate;
    private HostCapabilities hostCaps = HostCapabilities.UNKNOWN;
    private final LimelightCryptoProvider cryptoProvider;
    private final SnapshotParser snapshotParser;
    private volatile ParsedSnapshotCache parsedSnapshotCache;

    public static final class LaunchTimeoutException extends IOException {
        public final int seconds;

        public LaunchTimeoutException(int seconds) {
            super("The host did not answer the launch within " + seconds + " seconds");
            this.seconds = seconds;
        }
    }

    public NvHTTP(ComputerDetails.AddressTuple address, int httpsPort, String uniqueId,
                  X509Certificate serverCert, LimelightCryptoProvider cryptoProvider) throws IOException {
        this(address, httpsPort, uniqueId, serverCert, cryptoProvider, NightjarHttpJni::parseServerInfoXml);
    }

    NvHTTP(ComputerDetails.AddressTuple address, int httpsPort, String uniqueId,
           X509Certificate serverCert, LimelightCryptoProvider cryptoProvider,
           SnapshotParser snapshotParser) throws IOException {
        if (address == null) throw new IOException("Missing host address");
        this.httpPort = address.port;
        this.serverCert = serverCert;
        this.cryptoProvider = cryptoProvider;
        this.snapshotParser = snapshotParser == null ? NightjarHttpJni::parseServerInfoXml : snapshotParser;
        byte[] clientCertDer = encodeCert(cryptoProvider == null ? null : cryptoProvider.getClientCertificate());
        byte[] clientKeyPkcs8 = encodeKey(cryptoProvider == null ? null : cryptoProvider.getClientPrivateKey());
        byte[] serverCertDer = encodeCert(serverCert);
        String deviceName = Build.MODEL != null && !Build.MODEL.isEmpty() ? Build.MODEL : "Nightjar";
        this.handle = NightjarHttpJni.clientCreate(address.address, address.port, httpsPort,
                uniqueId == null ? "" : uniqueId, clientCertDer, clientKeyPkcs8, serverCertDer, deviceName);
        if (this.handle <= 0) {
            throw new IOException("Invalid host address or identity: " + address.address);
        }
        this.reaper = HandleReaper.register(this, handle, NightjarHttpJni::clientDestroy);
    }

    private static byte[] encodeCert(X509Certificate cert) throws IOException {
        if (cert == null) return new byte[0];
        try { return cert.getEncoded(); }
        catch (CertificateEncodingException e) { throw new IOException("Invalid certificate", e); }
    }

    private static byte[] encodeKey(PrivateKey key) {
        return key == null ? new byte[0] : key.getEncoded();
    }

    long httpHandle() { return handle; }

    public void setHostCapabilities(HostCapabilities caps) {
        this.hostCaps = caps == null ? HostCapabilities.UNKNOWN : caps;
    }

    public HostCapabilities getHostCapabilities() {
        return hostCaps;
    }

    void setServerCert(X509Certificate serverCert) {
        this.serverCert = serverCert;
        try {
            NightjarHttpJni.clientSetServerCertDer(handle, encodeCert(serverCert));
        } catch (IOException e) {
            throw new IllegalStateException(e);
        }
    }

    public boolean serverInfoFromAnotherCertificate() { return serverInfoFromAnotherCertificate; }

    public void setHttpsPort(int httpsPort) {
        NightjarHttpJni.clientSetHttpsPort(handle, httpsPort);
    }

    public int getHttpsPort() {
        return NightjarHttpJni.clientGetHttpsPort(handle);
    }

    public static void setSecondScreen(boolean second) {
        NightjarHttpJni.setSecondScreen(second);
    }

    public void cancelPendingRequests() {
        NightjarHttpJni.clientCancelPendingRequests(handle);
    }

    @Override
    public void close() {
        reaper.close();
    }

    public String getServerInfo(boolean likelyOnline) throws IOException, XmlPullParserException {
        serverInfoFromAnotherCertificate = false;
        Json.Obj ok = unwrapOkObj(NightjarHttpJni.getServerInfo(handle, likelyOnline), true);
        serverInfoFromAnotherCertificate = "cert_mismatch".equals(ok.optString("path"));
        try {
            return ok.getObject("info").getString("raw");
        } catch (Json.SyntaxException e) {
            throw new XmlPullParserException(e.getMessage());
        }
    }

    public ComputerDetails getComputerDetails(boolean likelyOnline) throws IOException, XmlPullParserException {
        serverInfoFromAnotherCertificate = false;
        Json.Obj ok = unwrapOkObj(NightjarHttpJni.getServerInfo(handle, likelyOnline), true);
        serverInfoFromAnotherCertificate = "cert_mismatch".equals(ok.optString("path"));
        try {
            return detailsFromSnapshot(ok.getObject("info"));
        } catch (Json.SyntaxException e) {
            throw new XmlPullParserException(e.getMessage());
        }
    }

    public ComputerDetails getComputerDetails(String serverInfo) throws IOException, XmlPullParserException {
        Json.Obj snap = unwrapOkObj(NightjarHttpJni.parseServerInfoXml(serverInfo, httpPort, true), true);
        return detailsFromSnapshot(snap);
    }

    public ComputerDetails probePairedComputer() throws IOException, XmlPullParserException {
        Json.Obj snap = unwrapOkObj(NightjarHttpJni.probePairedComputer(handle), true);
        return detailsFromSnapshot(snap);
    }

    static void readClientCapabilities(ComputerDetails details, String serverInfo)
            throws IOException, XmlPullParserException {
        String xml = serverInfo == null ? "" : serverInfo;
        try {
            Json.Obj snap = unwrapOkObj(NightjarHttpJni.parseServerInfoXml(xml, DEFAULT_HTTP_PORT, false), true);
            NvHttpJson.applyCapabilities(details, snap);
        } catch (Json.SyntaxException e) {
            throw new XmlPullParserException(e.getMessage());
        }
    }

    private ComputerDetails detailsFromSnapshot(Json.Obj snap) throws XmlPullParserException {
        ComputerDetails details = NvHttpJson.detailsFromSnapshot(snap, httpPort);
        hostCaps = details.caps;
        return details;
    }

    /**
     * Populates session, LAN/WAN endpoints, capabilities, and host family flags from a
     * Rust {@code server_info_json} object produced by {@code HostManager}.
     */
    public static void applyServerInfoSnapshot(ComputerDetails target, Json.Obj snap, int httpPort) {
        NvHttpJson.applyServerInfoSnapshot(target, snap, httpPort);
    }

    private Json.Obj parseSnapshot(String serverInfo) throws XmlPullParserException, IOException {
        ParsedSnapshotCache cached = parsedSnapshotCache;
        if (cached != null && serverInfo != null && serverInfo.equals(cached.xml)) {
            return cached.snapshot;
        }
        Json.Obj snap = unwrapOkObj(snapshotParser.parse(serverInfo, httpPort, false), true);
        if (serverInfo != null) {
            parsedSnapshotCache = new ParsedSnapshotCache(serverInfo, snap);
        }
        return snap;
    }

    public String getServerVersion(String serverInfo) throws XmlPullParserException, IOException {
        return NvHttpJson.optNullString(parseSnapshot(serverInfo), "app_version");
    }

    public PairingManager.PairState getPairState() throws IOException, XmlPullParserException {
        return getComputerDetails(true).pairState;
    }

    public PairingManager.PairState getPairState(String serverInfo) throws IOException, XmlPullParserException {
        return parseSnapshot(serverInfo).optBoolean("paired")
                ? PairingManager.PairState.PAIRED : PairingManager.PairState.NOT_PAIRED;
    }

    public long getMaxLumaPixelsH264(String serverInfo) throws XmlPullParserException, IOException {
        return parseSnapshot(serverInfo).optLong("max_luma_pixels_h264", 0);
    }

    public long getMaxLumaPixelsHEVC(String serverInfo) throws XmlPullParserException, IOException {
        return parseSnapshot(serverInfo).optLong("max_luma_pixels_hevc", 0);
    }

    public long getServerCodecModeSupport(String serverInfo) throws XmlPullParserException, IOException {
        return parseSnapshot(serverInfo).optLong("server_codec_mode_support", 0);
    }

    public String getGpuType(String serverInfo) throws XmlPullParserException, IOException {
        return NvHttpJson.optNullString(parseSnapshot(serverInfo), "gpu_type");
    }

    public boolean supports4K(String serverInfo) throws XmlPullParserException, IOException {
        return parseSnapshot(serverInfo).optBoolean("supports_4k", false);
    }

    public int getCurrentGame(String serverInfo) throws IOException, XmlPullParserException {
        return parseSnapshot(serverInfo).optInt("current_game", 0);
    }

    public int getHttpsPort(String serverInfo) {
        try { return parseSnapshot(serverInfo).optInt("https_port", 47984); }
        catch (Exception e) { return 47984; }
    }

    public int getExternalPort(String serverInfo) {
        try { return parseSnapshot(serverInfo).optInt("external_port", httpPort); }
        catch (Exception e) { return httpPort; }
    }

    public int[] getServerAppVersionQuad(String serverInfo) throws XmlPullParserException, IOException {
        String ver = getServerVersion(serverInfo);
        if (ver == null) throw new XmlPullParserException("Missing server version field");
        String[] parts = ver.split("\\.");
        if (parts.length != 4) throw new XmlPullParserException("Malformed server version field: " + ver);
        int[] quad = new int[4];
        try {
            for (int i = 0; i < 4; i++) {
                int n = Integer.parseInt(parts[i]);
                if (n < (i == 3 ? -1 : 0)) {
                    throw new XmlPullParserException("Malformed server version field: " + ver);
                }
                quad[i] = n;
            }
        } catch (NumberFormatException e) {
            throw new XmlPullParserException("Malformed server version field: " + ver);
        }
        return quad;
    }

    public int getServerMajorVersion(String serverInfo) throws XmlPullParserException, IOException {
        return getServerAppVersionQuad(serverInfo)[0];
    }

    public HardwareMetrics getHardwareMetrics() throws IOException {
        try {
            Json.Obj ok = unwrapOkObj(NightjarHttpJni.getHardwareMetrics(handle, hostCaps.toJson()), false);
            return HardwareMetrics.fromJsonObj(ok);
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException("Invalid hardware metrics response", e);
        }
    }

    public NetworkPathInfo getNetworkPath() throws IOException {
        try {
            Json.Obj reply = NvHttpJson.unwrapOk(NightjarHttpJni.getNetworkPath(handle, hostCaps.toJson()), false);
            return new NetworkPathInfo(NetworkPathInfo.Kind.valueOf(reply.getString("ok")));
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException("Invalid route response", e);
        }
    }

    public String getInternetPlay() throws IOException {
        try {
            return NvHttpJson.unwrapOk(NightjarHttpJni.getInternetPlay(handle, hostCaps.toJson()), false).getString("ok");
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException("Invalid internet play response", e);
        }
    }

    public boolean supportsFoundationAbr() throws IOException {
        try {
            return NvHttpJson.unwrapOk(NightjarHttpJni.supportsFoundationAbr(handle), false).optBoolean("ok", false);
        } catch (XmlPullParserException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public int configureFoundationAbr(org.json.JSONObject request) throws IOException {
        try {
            Json.Obj req = Json.obj().put("enabled", request.optBoolean("enabled", false));
            if (request.has("mode") && !request.isNull("mode")) {
                req.put("mode", request.optString("mode", "balanced"));
            }
            if (request.has("minBitrate") && !request.isNull("minBitrate")) {
                req.put("min_bitrate", request.optInt("minBitrate", 0));
            } else if (request.has("min_bitrate") && !request.isNull("min_bitrate")) {
                req.put("min_bitrate", request.optInt("min_bitrate", 0));
            }
            if (request.has("maxBitrate") && !request.isNull("maxBitrate")) {
                req.put("max_bitrate", request.optInt("maxBitrate", 0));
            } else if (request.has("max_bitrate") && !request.isNull("max_bitrate")) {
                req.put("max_bitrate", request.optInt("max_bitrate", 0));
            }
            Json.Obj reply = unwrapOkObj(NightjarHttpJni.configureFoundationAbr(handle, req.toString()), false);
            if (!reply.optBoolean("success", false)) {
                throw new IOException(reply.optString("error", "ABR configuration rejected"));
            }
            return Math.max(0, reply.optInt("initial_bitrate", 0));
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public org.json.JSONObject sendFoundationAbrFeedback(org.json.JSONObject feedback) throws IOException {
        try {
            Json.Obj req = Json.obj()
                    .put("packet_loss", feedback.optDouble("packetLoss", 0))
                    .put("rtt_ms", feedback.optDouble("rttMs", 0))
                    .put("decode_fps", feedback.optDouble("decodeFps", 0))
                    .put("dropped_frames", feedback.optInt("droppedFrames", 0))
                    .put("current_bitrate", feedback.optInt("currentBitrate", 0));
            Json.Obj raw = unwrapOkObj(NightjarHttpJni.sendFoundationAbrFeedback(handle, req.toString()), false);
            org.json.JSONObject out = new org.json.JSONObject();
            if (!raw.isNull("new_bitrate")) out.put("newBitrate", raw.getInt("new_bitrate"));
            out.put("bitrateApplied", raw.optBoolean("bitrate_applied", false));
            if (!raw.isNull("bitrate_apply_error")) out.put("bitrateApplyError", raw.getString("bitrate_apply_error"));
            out.put("reason", raw.optString("reason", ""));
            return out;
        } catch (XmlPullParserException | org.json.JSONException | Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public double probePyroWaveBandwidth(ComputerDetails details, IntConsumer progress) throws IOException {
        if (serverCert == null || details == null || !details.supportsPyroWaveBandwidthProbe()) {
            throw new IOException("This paired host does not advertise the bandwidth test");
        }
        return probePyroWaveBandwidthBytes(details.pyroWaveBandwidthProbeBytes, progress);
    }

    double probePyroWaveBandwidthBytes(int expectedBytes, IntConsumer progress) throws IOException {
        try {
            NightjarHttpJni.PercentProgress cb = progress == null ? null : progress::accept;
            return NvHttpJson.unwrapOk(NightjarHttpJni.probeBandwidth(handle, expectedBytes, cb), false)
                    .getDouble("ok");
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public boolean supportsFoundationBandwidthTest() throws IOException {
        try {
            return NvHttpJson.unwrapOk(NightjarHttpJni.supportsFoundationBandwidthTest(handle), false).optBoolean("ok", false);
        } catch (XmlPullParserException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public double probeFoundationBandwidth(IntConsumer progress) throws IOException {
        try {
            NightjarHttpJni.PercentProgress cb = progress == null ? null : progress::accept;
            return unwrapOkObj(NightjarHttpJni.probeFoundationBandwidth(handle, cb), false).getDouble("bandwidth_mbps");
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public static int probeStartKbps(int configuredKbps, double probeMbps) {
        long share = Math.round(Math.max(0.0, probeMbps) * 1000.0 * 0.8);
        return (int) Math.max(Math.min(2000, configuredKbps), Math.min(configuredKbps, share));
    }

    public double probeLinkMbps() throws IOException {
        try {
            return NvHttpJson.unwrapOk(NightjarHttpJni.probeLinkMbps(handle), false).getDouble("ok");
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public PyroWavePace.Result probePyroWaveUdp(int kbps, int packetSize, int fps, int paceKbps) throws IOException {
        try {
            return PyroWavePace.Result.fromJsonObj(unwrapOkObj(
                    NightjarHttpJni.probePyroWaveUdp(handle, kbps, packetSize, fps, paceKbps), false));
        } catch (XmlPullParserException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public double probeServerLatency() throws IOException, XmlPullParserException {
        try {
            return NvHttpJson.unwrapOk(NightjarHttpJni.probeServerLatency(handle, SHORT_CONNECTION_TIMEOUT), true)
                    .getDouble("ok");
        } catch (Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public PairingManager getPairingManager() {
        return new PairingManager(this, cryptoProvider);
    }

    static String stripLegacyOrderPrefix(String title) {
        if (title == null) return "";
        int i = 0;
        while (i < title.length() && (title.charAt(i) == '\u200B' || title.charAt(i) == '\u200C')) i++;
        return title.substring(i);
    }

    public static LinkedList<NvApp> getAppListByReader(Reader r) throws XmlPullParserException, IOException {
        return getAppListByReader(r, HostCapabilities.UNKNOWN);
    }

    public static LinkedList<NvApp> getAppListByReader(Reader r, HostCapabilities caps)
            throws XmlPullParserException, IOException {
        StringBuilder sb = new StringBuilder();
        char[] buf = new char[4096];
        int n;
        while ((n = r.read(buf)) != -1) sb.append(buf, 0, n);
        try {
            Json.Arr arr = NvHttpJson.unwrapOk(NightjarHttpJni.parseAppListXml(sb.toString(), caps.toJson()), true)
                    .getArray("ok");
            LinkedList<NvApp> list = new LinkedList<>();
            boolean monitorCatalogue = false;
            for (int i = 0; i < arr.length(); i++) {
                Json.Obj o = arr.getObject(i);
                NvApp app = new NvApp(o.getString("name"), o.getInt("id"), o.getBoolean("hdr_supported"));
                app.setHostCapabilities(caps);
                app.setAppUuid(o.optString("uuid", ""));
                if (!o.isNull("host_index")) app.setHostIndex(String.valueOf(o.getInt("host_index")));
                app.setArtVersion(o.optString("art_version", ""));
                app.setMonitorResume(o.optBoolean("monitor_resume", false));
                monitorCatalogue |= app.getControl() == NvApp.Control.DISCONNECT_MONITOR;
                list.addLast(app);
            }
            for (NvApp app : list) app.setMonitorResume(monitorCatalogue);
            return list;
        } catch (Json.SyntaxException e) {
            throw new XmlPullParserException(e.getMessage());
        }
    }

    public String getAppListRaw() throws IOException {
        try {
            return NvHttpJson.unwrapOk(NightjarHttpJni.getAppListRaw(handle), false).getString("ok");
        } catch (XmlPullParserException | Json.SyntaxException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    public LinkedList<NvApp> getAppList() throws HostHttpResponseException, IOException, XmlPullParserException {
        return getAppListByReader(new java.io.StringReader(getAppListRaw()), hostCaps);
    }

    public NvApp getAppById(int appId) throws IOException, XmlPullParserException {
        for (NvApp app : getAppList()) if (app.getAppId() == appId) return app;
        return null;
    }

    public NvApp getAppByUuid(String appUuid) throws IOException, XmlPullParserException {
        if (appUuid == null || appUuid.isEmpty()) return null;
        for (NvApp app : getAppList()) if (appUuid.equals(app.getAppUuid())) return app;
        return null;
    }

    public NvApp getAppByName(String appName) throws IOException, XmlPullParserException {
        for (NvApp app : getAppList()) if (app.getAppName().equalsIgnoreCase(appName)) return app;
        return null;
    }

    public void unpair() throws IOException {
        try { NvHttpJson.unwrapOk(NightjarHttpJni.unpair(handle), false); }
        catch (XmlPullParserException e) { throw new IOException(e.getMessage(), e); }
    }

    public InputStream getBoxArt(NvApp app) throws IOException {
        byte[] bytes = NightjarHttpJni.getBoxArt(handle, app.getAppId());
        if (bytes == null) throw new FileNotFoundException("Box art not found");
        return new ByteArrayInputStream(bytes);
    }

    public static HostDisplayChoice launchDisplayChoice(ConnectionContext context, NvApp.Role role) {
        return NvHttpLaunch.launchDisplayChoice(context, role);
    }

    public static boolean wantsVirtualDisplay(ConnectionContext context, NvApp.Role role) {
        return NvHttpLaunch.wantsVirtualDisplay(context, role);
    }

    public static int getStreamFrameRateMilliHz(ConnectionContext context) {
        return NvHttpLaunch.getStreamFrameRateMilliHz(context);
    }

    public static boolean asksClipboardSync(ConnectionContext context) {
        return NvHttpLaunch.asksClipboardSync(context);
    }

    static String getLaunchQuery(ConnectionContext context, int appId, boolean enableHdr) {
        return NvHttpLaunch.getLaunchQuery(context, appId, enableHdr);
    }

    public boolean launchApp(ConnectionContext context, String verb, int appId, boolean enableHdr)
            throws IOException, XmlPullParserException {
        return launchApp(context, verb, appId, enableHdr, LAUNCH_READ_TIMEOUT_SECONDS * 1000L);
    }

    boolean launchApp(ConnectionContext context, String verb, int appId, boolean enableHdr, long readTimeoutMs)
            throws IOException, XmlPullParserException {
        String req = NvHttpLaunch.buildLaunchRequestJson(context, appId, enableHdr);
        Json.Obj ok = unwrapOkObj(NightjarHttpJni.launchApp(
                handle, verb, req, context.hostCaps.toJson(), context.isNvidiaServerSoftware,
                (int) Math.max(1, Math.min(Integer.MAX_VALUE, readTimeoutMs))), true);
        if (ok.optBoolean("confirming", false)) {
            throw new HostHttpResponseException(ok.optInt("status", 0), ok.optString("body", ""));
        }
        if (!ok.optBoolean("ok", false)) return false;
        context.rtspSessionUrl = NvHttpJson.optNullString(ok, "session_url");
        // Checked and cleaned by the shared parser (launch_warning.rs); shown once after connecting.
        context.launchWarningCode = NvHttpJson.optNullString(ok, "warning_code");
        context.launchWarningMessage = NvHttpJson.optNullString(ok, "warning_message");
        return true;
    }

    public boolean disconnectRole(NvApp.Role role) throws IOException, XmlPullParserException {
        return NvHttpJson.unwrapOk(NightjarHttpJni.disconnectRole(handle, role.ordinal()), true, true).optBoolean("ok", false);
    }

    public boolean quitApp() throws IOException, XmlPullParserException {
        return NvHttpJson.unwrapOk(NightjarHttpJni.quitApp(handle), true).optBoolean("ok", false);
    }

    public String getClipboard() throws IOException {
        try { return NvHttpJson.unwrapOk(NightjarHttpJni.getClipboard(handle), false).getString("ok"); }
        catch (XmlPullParserException | Json.SyntaxException e) { throw new IOException(e.getMessage(), e); }
    }

    public int setBitrate(int kbps) throws IOException, XmlPullParserException {
        try { return NvHttpJson.unwrapOk(NightjarHttpJni.setBitrate(handle, kbps), false, true).getInt("ok"); }
        catch (Json.SyntaxException e) { throw new IOException(e.getMessage(), e); }
    }

    public void sendClipboard(String text) throws IOException {
        try { NvHttpJson.unwrapOk(NightjarHttpJni.sendClipboard(handle, text), false); }
        catch (XmlPullParserException e) { throw new IOException(e.getMessage(), e); }
    }

    public boolean getClipboardImage(OutputStream out) throws IOException {
        IOException[] sinkErr = new IOException[1];
        NightjarHttpJni.ByteSink sink = (buf, off, len) -> {
            try { out.write(buf, off, len); }
            catch (IOException e) { sinkErr[0] = e; throw e; }
        };
        try {
            Json.Obj reply = NvHttpJson.unwrapClipboard(NightjarHttpJni.getClipboardImage(handle, sink), sinkErr[0]);
            return !reply.isNull("ok");
        } catch (Json.SyntaxException e) { throw new IOException(e); }
    }

    public void sendClipboardImage(File png) throws IOException {
        try { NvHttpJson.unwrapClipboard(NightjarHttpJni.sendClipboardImageFile(handle, png.getAbsolutePath()), null); }
        catch (Json.SyntaxException e) { throw new IOException(e); }
    }

    static Json.Obj unwrapOkObj(String raw, boolean xmlErrors) throws IOException, XmlPullParserException {
        return NvHttpJson.unwrapOkObj(raw, xmlErrors);
    }
}
