package com.limelight.nvstream;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.NvHTTPRoleTest;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;

import java.io.IOException;
import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Consumer;

import static org.junit.Assert.*;

public class NvConnectionRoleTest {
    @Test
    public void recoveryNeverLaunchesQuitsOrConfirmsReplacement() throws Exception {
        AtomicInteger requests = new AtomicInteger();
        ConnectionContext context = context(73, request -> fail("Recovery must never confirm replacement"), new AtomicInteger());
        context.streamConfig = new StreamConfiguration.Builder()
                .setApp(new NvApp("Game", 73, false)).setResumeOnly(true).build();
        NvHTTP http = http(context, "resume", requests, NvHTTPRoleTest.error("replace-confirm"));
        assertThrows(HostHttpResponseException.class, () -> NvConnection.launchApp(http, context, "launch"));
        assertEquals(0, requests.get());
        NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "device", context.streamConfig, null, null);
        assertThrows(HostHttpResponseException.class, () -> connection.quitAndLaunch(http, context));
        assertEquals(0, requests.get());
        assertThrows(HostHttpResponseException.class, () -> NvConnection.launchApp(http, context, "resume"));
        assertEquals(1, requests.get());
        assertTrue(NvConnection.launchApp(http, context, "resume"));
        assertEquals(2, requests.get());
    }

    private ConnectionContext context(int appId, Consumer<LaunchConfirmation> decide, AtomicInteger completed) {
        ConnectionContext context = new ConnectionContext();
        context.streamConfig = new StreamConfiguration.Builder().setApp(new NvApp("Tile", appId, false)).build();
        context.connListener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("launchConfirmationRequired")) {
                        decide.accept((LaunchConfirmation) args[0]);
                    } else if (method.getName().equals("launchActionCompleted")) {
                        completed.incrementAndGet();
                    }
                    return null;
                });
        return context;
    }

    private NvHTTP http(ConnectionContext expected, String verb, AtomicInteger requests,
                        HostHttpResponseException... errors) throws IOException {
        return http(expected, verb, requests, new ArrayList<>(), errors);
    }

    // Records each request's ConnectionContext.confirmReplace in confirmed.
    private NvHTTP http(ConnectionContext expected, String verb, AtomicInteger requests, List<Boolean> confirmed,
                        HostHttpResponseException... errors) throws IOException {
        return new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device", null,
                new LimelightCryptoProvider() {
                    public java.security.cert.X509Certificate getClientCertificate() { return null; }
                    public java.security.PrivateKey getClientPrivateKey() { return null; }
                    public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
                    public String encodeBase64String(byte[] data) { return ""; }
                }) {
            @Override
            public boolean launchApp(ConnectionContext context, String actualVerb, int appId, boolean hdr)
                    throws IOException {
                assertSame(expected, context);
                assertEquals(verb, actualVerb);
                assertEquals(expected.streamConfig.getApp().getAppId(), appId);
                confirmed.add(context.confirmReplace);
                int index = requests.getAndIncrement();
                if (index < errors.length) {
                    throw errors[index];
                }
                return true;
            }

            @Override
            public boolean quitApp() {
                throw new AssertionError("Role tile attempted to cancel the main session");
            }
        };
    }

    @Test
    public void interruptedConfirmationNeverRetriesAndClosesThePrompt() throws Exception {
        AtomicInteger requests = new AtomicInteger();
        ConnectionContext context = context(73, request -> Thread.currentThread().interrupt(), new AtomicInteger());
        NvHTTP http = http(context, "launch", requests, NvHTTPRoleTest.error("replace-confirm"));
        AtomicInteger finished = new AtomicInteger();
        NvConnectionListener listener = context.connListener;
        context.connListener = (NvConnectionListener) Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("launchConfirmationFinished")) finished.incrementAndGet();
                    return method.invoke(listener, args);
                });
        try {
            assertEquals(499, assertThrows(NvConnection.LocalLaunchFailure.class,
                    () -> NvConnection.launchApp(http, context, "launch")).getErrorCode());
            assertTrue(Thread.currentThread().isInterrupted());
            assertEquals(1, requests.get());
            assertEquals(1, finished.get());
        } finally {
            Thread.interrupted();
        }
    }

    @Test
    public void remoteRoleAndUnknown410NeverBecomeReplacementConfirmations() throws Exception {
        for (int appId : new int[]{2147483505, 2147483506, 73}) {
            AtomicInteger requests = new AtomicInteger();
            ConnectionContext context = context(appId, request -> fail("Unexpected confirmation"), new AtomicInteger());
            HostHttpResponseException error = appId == 73 ? new HostHttpResponseException(410, "Gone") :
                    NvHTTPRoleTest.error("replace-confirm");
            NvHTTP http = http(context, "launch", requests, error);
            assertSame(error, assertThrows(HostHttpResponseException.class,
                    () -> NvConnection.launchApp(http, context, "launch")));
            assertEquals(1, requests.get());
            assertFalse(context.launchActionCompleted);
        }
    }

    @Test
    public void launchAndResumeRepeatTheSameRequestOnlyAfterConfirmation() throws Exception {
        for (String verb : new String[] {"launch", "resume"}) {
            AtomicInteger requests = new AtomicInteger();
            ConnectionContext context = context(73, request -> {
                assertEquals(1, requests.get());
                request.confirm(LaunchConfirmation.nowMs());
            }, new AtomicInteger());
            assertTrue(NvConnection.launchApp(http(context, verb, requests,
                    NvHTTPRoleTest.error("replace-confirm")), context, verb));
            assertEquals(2, requests.get());
            assertFalse(context.launchActionCompleted);
        }
    }

    @Test
    public void onlyTheRetryAfterAConfirmedReplaceConfirmsIt() throws Exception {
        // Confirmed: the first request asks nothing, the retry confirms, and nothing after it does.
        HostHttpResponseException denied = new HostHttpResponseException(403, "Permission denied");
        for (HostHttpResponseException retryError : new HostHttpResponseException[] {null, denied}) {
            List<Boolean> confirmed = new ArrayList<>();
            ConnectionContext context = context(73, request -> request.confirm(LaunchConfirmation.nowMs()),
                    new AtomicInteger());
            NvHTTP http = retryError == null ?
                    http(context, "launch", new AtomicInteger(), confirmed, NvHTTPRoleTest.error("replace-confirm")) :
                    http(context, "launch", new AtomicInteger(), confirmed, NvHTTPRoleTest.error("replace-confirm"),
                            retryError);
            if (retryError == null) {
                assertTrue(NvConnection.launchApp(http, context, "launch"));
            } else {
                assertSame(retryError, assertThrows(HostHttpResponseException.class,
                        () -> NvConnection.launchApp(http, context, "launch")));
            }
            assertEquals(Arrays.asList(false, true), confirmed);
            assertFalse(context.confirmReplace);
        }
        // Cancelled or timed out: no retry, so nothing confirms.
        for (boolean timeout : new boolean[] {false, true}) {
            List<Boolean> confirmed = new ArrayList<>();
            ConnectionContext context = context(73, request -> {
                if (timeout) {
                    request.getState(LaunchConfirmation.nowMs() + 60_000);
                } else {
                    request.cancel();
                }
            }, new AtomicInteger());
            NvHTTP http = http(context, "launch", new AtomicInteger(), confirmed, NvHTTPRoleTest.error("replace-confirm"));
            assertThrows(IOException.class, () -> NvConnection.launchApp(http, context, "launch"));
            assertEquals(Collections.singletonList(false), confirmed);
            assertFalse(context.confirmReplace);
        }
        // A confirmed Terminate is not a replace.
        List<Boolean> confirmed = new ArrayList<>();
        ConnectionContext context = context(2147483504, request -> request.confirm(LaunchConfirmation.nowMs()),
                new AtomicInteger());
        assertFalse(NvConnection.launchApp(http(context, "launch", new AtomicInteger(), confirmed,
                NvHTTPRoleTest.error("terminate-confirm"), NvHTTPRoleTest.error("terminate-done")), context, "launch"));
        assertEquals(Arrays.asList(false, false), confirmed);
    }

    @Test
    public void cancelledAndExpiredPromptsNeverRetry() throws Exception {
        for (boolean timeout : new boolean[] {false, true}) {
            AtomicInteger requests = new AtomicInteger();
            ConnectionContext context = context(73, request -> {
                if (timeout) {
                    request.getState(LaunchConfirmation.nowMs() + 60_000);
                } else {
                    request.cancel();
                }
            }, new AtomicInteger());
            NvHTTP http = http(context, "launch", requests, NvHTTPRoleTest.error("replace-confirm"));
            IOException error = assertThrows(IOException.class, () -> NvConnection.launchApp(http, context, "launch"));
            assertTrue(error.getMessage().contains(timeout ? "timed out" : "cancelled"));
            assertEquals(1, requests.get());
        }
    }

    @Test
    public void denialAfterConfirmationIsReadableAndNeverRetried() throws Exception {
        AtomicInteger requests = new AtomicInteger();
        ConnectionContext context = context(73, request -> request.confirm(LaunchConfirmation.nowMs()), new AtomicInteger());
        HostHttpResponseException denied = new HostHttpResponseException(403, "Permission denied");
        NvHTTP http = http(context, "launch", requests, NvHTTPRoleTest.error("replace-confirm"), denied);
        assertSame(denied, assertThrows(HostHttpResponseException.class,
                () -> NvConnection.launchApp(http, context, "launch")));
        assertEquals(2, requests.get());
    }

    @Test
    public void changedHostGenerationRequiresStartingAgainInsteadOfAnotherAutomaticConfirmation() throws Exception {
        AtomicInteger requests = new AtomicInteger();
        AtomicInteger prompts = new AtomicInteger();
        ConnectionContext context = context(73, request -> {
            prompts.incrementAndGet();
            request.confirm(LaunchConfirmation.nowMs());
        }, new AtomicInteger());
        HostHttpResponseException confirm = NvHTTPRoleTest.error("replace-confirm");
        NvHTTP http = http(context, "launch", requests, confirm, confirm);
        IOException error = assertThrows(IOException.class, () -> NvConnection.launchApp(http, context, "launch"));
        assertTrue(error.getMessage().contains("host session changed"));
        assertEquals(2, requests.get());
        assertEquals(1, prompts.get());
    }

    @Test
    public void terminationConfirmationAndCompletionDoNotStartAStream() throws Exception {
        AtomicInteger requests = new AtomicInteger();
        AtomicInteger completed = new AtomicInteger();
        ConnectionContext context = context(2147483504, request -> {
            assertTrue(request.isTerminate());
            request.confirm(LaunchConfirmation.nowMs());
        }, completed);
        assertFalse(NvConnection.launchApp(http(context, "launch", requests,
                NvHTTPRoleTest.error("terminate-confirm"), NvHTTPRoleTest.error("terminate-done")), context, "launch"));
        assertEquals(2, requests.get());
        assertEquals(1, completed.get());
        assertTrue(context.launchActionCompleted);
    }

    @Test
    public void vibepolloTerminationFromEitherTileCompletesAfterConfirmation() throws Exception {
        // Vibepollo asks a secondary client to confirm with Rubylight's words, then reports its own.
        HostHttpResponseException done = new HostHttpResponseException(410,
                "Active stream terminated. Remote Monitor and Remote Input remain connected.");
        for (int id : new int[] {2147483504, 2147483514}) {
            AtomicInteger requests = new AtomicInteger();
            AtomicInteger completed = new AtomicInteger();
            ConnectionContext context = context(id, request -> request.confirm(LaunchConfirmation.nowMs()), completed);
            assertFalse(NvConnection.launchApp(http(context, "launch", requests,
                    NvHTTPRoleTest.error("terminate-confirm"), done), context, "launch"));
            assertEquals(2, requests.get());
            assertEquals(1, completed.get());
            assertTrue(context.launchActionCompleted);
        }
    }

    @Test
    public void disconnect410CompletesWithoutPromptOrRetry() throws Exception {
        AtomicInteger requests = new AtomicInteger();
        AtomicInteger completed = new AtomicInteger();
        ConnectionContext context = context(2147483502, request -> fail("Disconnect is not a confirmation"), completed);
        assertFalse(NvConnection.launchApp(http(context, "launch", requests,
                NvHTTPRoleTest.error("monitor-disconnected")), context, "launch"));
        assertEquals(1, requests.get());
        assertEquals(1, completed.get());
    }

    @Test
    public void interruptionWhileWaitingNeverRetriesAndClosesThePrompt() throws Exception {
        AtomicInteger requests = new AtomicInteger();
        ConnectionContext context = context(73, request -> Thread.currentThread().interrupt(), new AtomicInteger());
        NvHTTP http = http(context, "launch", requests, NvHTTPRoleTest.error("replace-confirm"));
        try {
            assertEquals(499, assertThrows(NvConnection.LocalLaunchFailure.class,
                    () -> NvConnection.launchApp(http, context, "launch")).getErrorCode());
            assertTrue(Thread.currentThread().isInterrupted());
            assertEquals(1, requests.get());
        } finally {
            Thread.interrupted();
        }
    }

    @Test
    public void allControlTilesBypassTheLegacyQuitBeforeLaunchPath() throws Exception {
        for (int id = 2147483501; id <= 2147483507; id++) {
            AtomicInteger requests = new AtomicInteger();
            ConnectionContext context = context(id, request -> fail("Unexpected prompt"), new AtomicInteger());
            NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                    47984, "device", context.streamConfig, null, null);
            assertTrue(connection.quitAndLaunch(http(context, "launch", requests), context));
            assertEquals(1, requests.get());
        }
    }

    @Test
    public void apolloRemoteInputEndsWithTheStreamInsteadOfADisconnectTile() throws Exception {
        for (String host : new String[] {"apollo", "vibepollo"}) {
            boolean apollo = host.equals("apollo");
            NvApp app = new NvApp("Remote Input", apollo ? 1557224430 : 2147483506, false);
            if (apollo) {
                app.setAppUuid("8CB5C136-DA67-4F99-B4A1-F9CD35005CF4");
            }
            AtomicInteger disconnects = new AtomicInteger();
            NvHTTP http = new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "device", null,
                    new LimelightCryptoProvider() {
                        public java.security.cert.X509Certificate getClientCertificate() { return null; }
                        public java.security.PrivateKey getClientPrivateKey() { return null; }
                        public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
                        public String encodeBase64String(byte[] data) { return ""; }
                    }) {
                @Override
                public boolean disconnectRole(NvApp.Role role) {
                    assertEquals(NvApp.Role.INPUT_ONLY, role);
                    disconnects.incrementAndGet();
                    return true;
                }
            };
            ComputerDetails details = http.getComputerDetails(TestHosts.serverInfo(host + "-serverinfo.xml"));
            app.setHostCapabilities(details.caps);
            assertEquals(host, NvApp.Role.INPUT_ONLY, app.getRole());
            NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                    47984, "device", new StreamConfiguration.Builder().setApp(app).build(), null, null) {
                @Override
                public ComputerDetails refreshHostDetails() {
                    return details;
                }
            };
            java.lang.reflect.Field field = NvConnection.class.getDeclaredField("http");
            field.setAccessible(true);
            field.set(connection, http);
            // Apollo ends Remote Input when the stream disconnects; /launch of a Vibepollo tile ID
            // would find no app there.
            connection.disconnectRemoteSession();
            assertEquals(host, apollo ? 0 : 1, disconnects.get());
        }
    }

    @Test
    public void quitAppRejectsRemoteRolesAndRevokedPermissionBeforeSendingCancel() {
        for (int appId : new int[] {2147483505, 2147483506, 73}) {
            StreamConfiguration config = new StreamConfiguration.Builder()
                    .setApp(new NvApp("Tile", appId, false)).build();
            NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                    47984, "device", config, null, null) {
                @Override
                public ComputerDetails refreshHostDetails() {
                    ComputerDetails details = new ComputerDetails();
                    details.permission = appId == 73 ? 0 : ComputerDetails.PERMISSION_LAUNCH;
                    return details;
                }
            };
            assertThrows(IOException.class, connection::quitApp);
        }
    }

    @Test
    public void monitorSuppressesEveryInputFamilyBeforeTheNativeBridge() {
        StreamConfiguration config = new StreamConfiguration.Builder()
                .setApp(new NvApp("Monitor", 2147483505, false)).build();
        NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "device", config, null, null);
        assertFalse(connection.canSendInput());
        connection.sendMouseMove((short) 1, (short) 1);
        connection.sendMouseButtonDown((byte) 1);
        connection.sendKeyboardInput((short) 1, (byte) 0, (byte) 0, (byte) 0);
        connection.sendUtf8Text("test");
        connection.sendControllerInput((short) 0, (short) 1, 0, (byte) 0, (byte) 0,
                (short) 0, (short) 0, (short) 0, (short) 0);
        connection.sendControllerBatteryEvent((byte) 0, (byte) 0, (byte) 0);
        int unsupported = com.limelight.nvstream.jni.MoonBridge.LI_ERR_UNSUPPORTED;
        assertEquals(unsupported, connection.sendControllerArrivalEvent((byte) 0, (short) 1, (byte) 0, 0, (short) 0));
        assertEquals(unsupported, connection.sendTouchEvent((byte) 0, 0, 0, 0, 0, 0, 0, (short) 0));
        assertEquals(unsupported, connection.sendPenEvent((byte) 0, (byte) 0, (byte) 0, 0, 0, 0, 0, 0, (short) 0, (byte) 0));
        assertEquals(unsupported, connection.sendControllerTouchEvent((byte) 0, (byte) 0, 0, 0, 0, 0));
        assertEquals(unsupported, connection.sendControllerMotionEvent((byte) 0, (byte) 0, 0, 0, 0));
    }
}
