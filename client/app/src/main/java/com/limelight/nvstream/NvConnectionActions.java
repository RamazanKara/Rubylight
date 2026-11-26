/**
 * Host details refresh, permission-gated session actions, clipboard sync, Foundation ABR,
 * native API actions, and server-command dispatch for {@link NvConnection}.
 */
package com.limelight.nvstream;

import com.limelight.LimeLog;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeAudio;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nativeapi.NativeSession;
import com.limelight.nvstream.http.ClipboardImageException;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.FoundationAbr;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.nvstream.jni.NightjarSessionJni;
import com.limelight.nvstream.wol.WakeTargets;

import org.json.JSONObject;
import org.xmlpull.v1.XmlPullParserException;

import java.io.File;
import java.io.IOException;
import java.io.OutputStream;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

final class NvConnectionActions {
    private static final long STREAM_MOVE_WAIT_MS = 1500;

    private NvConnectionActions() {
    }

    static int roleOrdinal(NvApp.Role role) {
        if (role == NvApp.Role.REMOTE_MONITOR) return 1;
        if (role == NvApp.Role.INPUT_ONLY) return 2;
        return 0;
    }

    static int streamPermissionBits(NvConnection conn, ComputerDetails details) {
        return NightjarSessionJni.streamPermissions(
                conn.stopRequested,
                conn.isMonkey,
                roleOrdinal(conn.context.streamConfig.getApp().getRole()),
                details.pairState == PairingManager.PairState.PAIRED,
                details.permission,
                details.caps.has(HostCapabilities.Feature.RUNTIME_BITRATE));
    }

    static ComputerDetails refreshHostDetails(NvConnection conn)
            throws IOException, XmlPullParserException {
        NvHTTP connection;
        synchronized (conn) {
            if (conn.stopRequested || conn.http == null) {
                throw new IOException("Stream is disconnected");
            }
            connection = conn.http;
        }
        ComputerDetails details = connection.getComputerDetails(true);
        if (details.pairState != PairingManager.PairState.PAIRED) {
            throw new IOException("Device is no longer paired with the host");
        }
        conn.hostDetails = details;
        return details;
    }

    static boolean hasEndedSession(NvConnection conn) {
        NvHTTP connection;
        synchronized (conn) {
            if (conn.stopRequested || conn.http == null) return false;
            connection = conn.http;
        }
        return hasEndedSession(conn, connection);
    }

    static boolean hasEndedSession(NvConnection conn, NvHTTP connection) {
        NvApp app = conn.context.streamConfig.getApp();
        if (conn.stopRequested || app.getRole() != NvApp.Role.STREAM || app.isControlAction()) {
            return false;
        }
        try {
            ComputerDetails details = connection.probePairedComputer();
            return NightjarSessionJni.serverinfoConfirmsEndedSession(
                    conn.stopRequested,
                    roleOrdinal(app.getRole()),
                    app.isControlAction(),
                    details.pairState == PairingManager.PairState.PAIRED,
                    details.hasPermission(
                            ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH),
                    details.sessionIdle,
                    details.runningGameId,
                    details.runningGameUuid);
        } catch (IOException | XmlPullParserException e) {
            return false;
        }
    }

    static String hostFailureReason(NvConnection conn) {
        NvHTTP connection;
        synchronized (conn) {
            if (conn.stopRequested || conn.http == null) return null;
            connection = conn.http;
        }
        try {
            String reason = connection.probePairedComputer().hostLastFailure;
            return reason == null || reason.isEmpty() ? null : reason;
        } catch (IOException | XmlPullParserException e) {
            LimeLog.warning("The host's failure reason could not be read: " + e);
            return null;
        }
    }

    static synchronized NvHTTP foundationAbrHttp(NvConnection conn) throws IOException {
        if (conn.foundationAbrHttp == null) {
            int httpsPort =
                    conn.hostDetails != null && conn.hostDetails.httpsPort > 0
                            ? conn.hostDetails.httpsPort
                            : conn.context.httpsPort;
            conn.foundationAbrHttp =
                    new NvHTTP(
                            conn.context.serverAddress,
                            httpsPort,
                            conn.uniqueId,
                            conn.context.serverCert,
                            conn.cryptoProvider);
        }
        return conn.foundationAbrHttp;
    }

    static int startFoundationAbr(NvConnection conn, int maxKbps) throws IOException {
        NvHTTP request = foundationAbrHttp(conn);
        if (!request.supportsFoundationAbr()) return -1;
        return request.configureFoundationAbr(FoundationAbr.enableRequest(maxKbps));
    }

    static void stopFoundationAbr(NvConnection conn) throws IOException {
        foundationAbrHttp(conn).configureFoundationAbr(FoundationAbr.disableRequest());
    }

    static String getClipboard(NvConnection conn) throws IOException, XmlPullParserException {
        if (!conn.refreshHostDetails().canReadClipboard()) {
            throw new IOException("Host clipboard read permission denied");
        }
        return conn.http.getClipboard();
    }

    static boolean getClipboardImage(NvConnection conn, OutputStream out)
            throws IOException, XmlPullParserException {
        if (!conn.nativeImageClipboard) {
            throw noImageClipboard();
        }
        if (!conn.refreshHostDetails().canReadClipboard()) {
            throw new IOException("Host clipboard read permission denied");
        }
        return conn.http.getClipboardImage(out);
    }

    private static IOException noImageClipboard() {
        return new ClipboardImageException(
                ClipboardImageException.Reason.NO_HOST_SUPPORT, "Host has no image clipboard");
    }

    static void quitApp(NvConnection conn) throws IOException, XmlPullParserException {
        ComputerDetails details = conn.refreshHostDetails();
        if ((streamPermissionBits(conn, details) & 8) == 0) {
            throw new IOException("The host denied permission to quit this app.");
        }
        if (!conn.http.quitApp()) {
            throw new IOException("The host did not quit the app.");
        }
    }

    static NativeSession nativeSessionAction(NvConnection conn, NativeSession.ActionId action)
            throws IOException, XmlPullParserException {
        NativeApiClient client = nativeClientForAction(conn);
        try {
            return client.sessionAction(action);
        } catch (NativeApiException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    private static NativeApiClient nativeClientForAction(NvConnection conn)
            throws IOException, XmlPullParserException {
        ComputerDetails details = conn.refreshHostDetails();
        if ((streamPermissionBits(conn, details) & 8) == 0) {
            throw new IOException("The host denied permission to change this session.");
        }
        if (!details.caps.has(HostCapabilities.Feature.NATIVE_API)) {
            throw new IOException("The host has no native session actions.");
        }
        NativeApiClient client = nativeClient(conn);
        if (client == null) {
            throw new IOException("The host's certificate is not known.");
        }
        return client;
    }

    static void closeGame(NvConnection conn) throws IOException, XmlPullParserException {
        if (conn.hasNativeSessionActions()) {
            try {
                if (nativeClientForAction(conn).closeGameIfAllowed()) {
                    return;
                }
            } catch (NativeApiException e) {
                throw new IOException(e.getMessage(), e);
            }
        }
        conn.quitApp();
    }

    static void openHostMenu(NvConnection conn) throws IOException {
        NativeApiClient client = conn.nativeApiClient();
        if (client == null) {
            throw new IOException("The host has no menu for this stream.");
        }
        boolean open;
        try {
            open = client.openCouchMenu();
        } catch (NativeApiException e) {
            throw new IOException(e.getMessage(), e);
        }
        if (!open) {
            throw new IOException("The host did not open its menu.");
        }
    }

    static NativeAudio hostAudio(NvConnection conn) throws IOException {
        NativeApiClient client = conn.nativeApiClient();
        if (client == null) {
            throw new IOException("The host has no audio settings for this stream.");
        }
        try {
            return client.audio();
        } catch (NativeApiException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    static NativeAudio writeHostAudio(NvConnection conn, String id, boolean on) throws IOException {
        NativeApiClient client = conn.nativeApiClient();
        if (client == null) {
            throw new IOException("The host has no audio settings for this stream.");
        }
        try {
            return client.writeAudio(id, on);
        } catch (NativeApiException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    static void refreshNativeHostControl(NvConnection conn, Runnable read) {
        if (conn.isMonkey || !conn.hasNativeSessionActions()) {
            return;
        }
        NativeApiClient client = nativeClient(conn);
        if (client == null) {
            return;
        }
        new Thread(
                        () -> {
                            try {
                                NativeCapabilities capabilities = client.capabilities();
                                conn.nativeHostControl = capabilities.hostControl;
                                conn.nativeCouchMenu =
                                        capabilities.has(NativeCapabilities.COUCH_MENU);
                                conn.nativeUsbForwarding = capabilities.offersUsbForwarding();
                                conn.nativeImageClipboard =
                                        capabilities.has(NativeCapabilities.CLIPBOARD_IMAGE);
                                conn.nativeSessionGuests =
                                        capabilities.has(NativeCapabilities.SESSION_GUESTS)
                                                || capabilities.has(
                                                        NativeCapabilities.GUEST_INVITES);
                                conn.nativeNightMode =
                                        capabilities.offersAudio(NativeCapabilities.NIGHT_MODE);
                                conn.nativeHeadphoneSurround =
                                        capabilities.offersAudio(
                                                NativeCapabilities.HEADPHONE_SURROUND);
                                conn.nativeQuickSuspend = capabilities.offersQuickSuspend();
                                if (read != null) read.run();
                                if (conn.appContext != null) {
                                    new WakeTargets(conn.appContext)
                                            .refresh(conn.hostDetails.uuid, capabilities, client);
                                }
                            } catch (NativeApiException e) {
                                LimeLog.info("Native capabilities not read: " + e.getMessage());
                            }
                        },
                        "Native capabilities")
                .start();
    }

    static String readStreamMovedTo(NvConnection conn, boolean recoveryFollows) throws IOException {
        if (!NightjarPolicyJni.continueHereReadsMoveAfterEnd(
                conn.offersContinueHere(), false, recoveryFollows)) {
            return null;
        }
        NativeApiClient client = nativeClient(conn);
        if (client == null) {
            return null;
        }
        AtomicReference<String> movedTo = new AtomicReference<>();
        AtomicBoolean answered = new AtomicBoolean();
        Thread read =
                new Thread(
                        () -> {
                            try {
                                movedTo.set(client.streamMove().movedTo);
                                answered.set(true);
                            } catch (NativeApiException e) {
                                LimeLog.info(
                                        "Could not ask the PC whether the stream moved: "
                                                + e.getMessage());
                            }
                        },
                        "Stream move");
        read.start();
        try {
            read.join(STREAM_MOVE_WAIT_MS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        if (!answered.get()) {
            throw new IOException("The PC did not say whether the stream moved");
        }
        return movedTo.get();
    }

    static NativeApiClient nativeClient(NvConnection conn) {
        HostEndpoint endpoint = conn.hostEndpoint();
        return endpoint == null ? null : new NativeApiClient(conn.nativeApiTransport, endpoint);
    }

    static HostEndpoint hostEndpoint(NvConnection conn) {
        return HostEndpoint.of(
                conn.hostDetails.uuid,
                conn.context.serverAddress,
                conn.context.httpsPort,
                conn.context.serverCert,
                conn.uniqueId,
                conn.cryptoProvider);
    }

    static void announceDisconnect(NvConnection conn) {
        if (conn.isMonkey
                || conn.context.streamConfig.getApp().getRole() != NvApp.Role.STREAM
                || !conn.hasNativeSessionActions()) {
            return;
        }
        NativeApiClient client = nativeClient(conn);
        if (client == null) {
            return;
        }
        new Thread(
                        () -> {
                            try {
                                client.sessionAction(NativeSession.ActionId.DISCONNECT);
                            } catch (NativeApiException e) {
                                LimeLog.info(
                                        "Native disconnect not delivered: " + e.getMessage());
                            }
                        },
                        "Native disconnect")
                .start();
    }

    static void disconnectRemoteSession(NvConnection conn)
            throws IOException, XmlPullParserException {
        ComputerDetails details = conn.refreshHostDetails();
        if ((streamPermissionBits(conn, details) & 16) == 0) {
            throw new IOException("The host denied permission to end this remote session.");
        }
        if (!details.caps.mayHave(HostCapabilities.Feature.REMOTE_SESSION_TILES)) {
            return;
        }
        if (!conn.http.disconnectRole(conn.context.streamConfig.getApp().getRole())) {
            throw new IOException("The host did not end the remote session.");
        }
    }

    static int setBitrate(NvConnection conn, int kbps) throws IOException, XmlPullParserException {
        ComputerDetails details = conn.refreshHostDetails();
        if ((streamPermissionBits(conn, details) & 4) == 0) {
            throw new IOException("Host does not allow runtime bitrate changes");
        }
        return conn.http.setBitrate(kbps);
    }

    static void sendClipboard(NvConnection conn, String text)
            throws IOException, XmlPullParserException {
        if (conn.context.streamConfig.getApp().getRole() == NvApp.Role.REMOTE_MONITOR
                || !conn.refreshHostDetails().canWriteClipboard()) {
            throw new IOException("Host clipboard write permission denied");
        }
        if (!conn.isMonkey) {
            conn.http.sendClipboard(text);
        }
    }

    static void sendClipboardImage(NvConnection conn, File png)
            throws IOException, XmlPullParserException {
        if (!conn.nativeImageClipboard) {
            throw noImageClipboard();
        }
        if (conn.context.streamConfig.getApp().getRole() == NvApp.Role.REMOTE_MONITOR
                || !conn.refreshHostDetails().canWriteClipboard()) {
            throw new IOException("Host clipboard write permission denied");
        }
        if (!conn.isMonkey) {
            conn.http.sendClipboardImage(png);
        }
    }

    static boolean sendServerCommand(NvConnection conn, int index, String name)
            throws IOException, XmlPullParserException {
        ComputerDetails details = conn.refreshHostDetails();
        boolean matches =
                index >= 0
                        && index < details.serverCommands.size()
                        && details.serverCommands.get(index).equals(name);
        boolean allowed =
                NightjarSessionJni.canRunServerCommand(
                        conn.stopRequested,
                        conn.isMonkey,
                        roleOrdinal(conn.context.streamConfig.getApp().getRole()),
                        details.pairState == PairingManager.PairState.PAIRED,
                        details.permission,
                        index,
                        details.serverCommands.size(),
                        matches);
        if (!allowed) {
            throw new IOException(
                    "Server command changed or permission denied. Reopen the stream menu.");
        }
        synchronized (MoonBridge.class) {
            if (conn.stopRequested || !conn.nativeConnected || conn.isMonkey) {
                return false;
            }
            long nowMs = System.nanoTime() / 1_000_000L;
            long prevMs = conn.lastServerCommandTime == 0 ? -1L : conn.lastServerCommandTime;
            if (!NightjarSessionJni.serverCommandThrottleAllows(prevMs, nowMs)) {
                throw new IOException("Wait one second between server commands");
            }
            conn.lastServerCommandTime = nowMs == 0 ? 1L : nowMs;
            return MoonBridge.sendServerCommand(details.serverCommandPayload(index));
        }
    }
}
