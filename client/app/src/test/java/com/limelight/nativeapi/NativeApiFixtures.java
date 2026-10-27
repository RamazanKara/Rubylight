package com.limelight.nativeapi;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.TestHosts;

import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509CertificateConverter;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;

import java.math.BigInteger;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Date;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** Hosts, a client identity and a scripted transport for the native API tests. */
public final class NativeApiFixtures {
    private NativeApiFixtures() {
    }

    private static final String RUBYLIGHT_PAIRED = "<RustHostVersion>3.0.0</RustHostVersion><PairStatus>1</PairStatus>";

    public static final HostCapabilities NATIVE_CAPS =
            TestHosts.caps("<root status_code=\"200\">" + RUBYLIGHT_PAIRED
                    + "<NativeApiVersion>1</NativeApiVersion></root>");

    private static KeyPair keys;
    private static X509Certificate certificate;

    /** One RSA identity for the whole test run; generating keys is slow. */
    public static synchronized X509Certificate certificate() {
        if (certificate == null) {
            try {
                KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
                generator.initialize(2048);
                keys = generator.generateKeyPair();
                X500Name name = new X500Name("CN=Native API test");
                certificate = new JcaX509CertificateConverter().getCertificate(
                        new JcaX509v3CertificateBuilder(name, BigInteger.ONE, new Date(0),
                                new Date(4102444800000L), name, keys.getPublic())
                                .build(new JcaContentSignerBuilder("SHA256withRSA").build(keys.getPrivate())));
            } catch (Exception e) {
                throw new IllegalStateException(e);
            }
        }
        return certificate;
    }

    public static LimelightCryptoProvider crypto() {
        X509Certificate cert = certificate();
        PrivateKey key = keys.getPrivate();
        return new LimelightCryptoProvider() {
            @Override public X509Certificate getClientCertificate() { return cert; }
            @Override public PrivateKey getClientPrivateKey() { return key; }
            @Override public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            @Override public String encodeBase64String(byte[] data) { return ""; }
        };
    }

    public static ComputerDetails host(String uuid, String name, HostCapabilities caps, boolean online) {
        ComputerDetails details = new ComputerDetails();
        details.uuid = uuid;
        details.name = name;
        details.caps = caps;
        details.state = online ? ComputerDetails.State.ONLINE : ComputerDetails.State.OFFLINE;
        details.pairState = PairingManager.PairState.PAIRED;
        details.activeAddress = new ComputerDetails.AddressTuple("10.0.0." + Math.abs(uuid.hashCode() % 200 + 1), 47989);
        details.httpsPort = 47984;
        details.serverCert = certificate();
        return details;
    }

    public static ComputerDetails nativeHost(String uuid, String name, boolean online) {
        return host(uuid, name, NATIVE_CAPS, online);
    }

    public static ComputerDetails classicHost(String uuid, String name) {
        return host(uuid, name, TestHosts.fixture("sunshine-serverinfo.xml"), true);
    }

    public static String game(String installId, int appId, String title, String gameId, boolean running) {
        return "{\"install_id\":\"" + installId + "\",\"app_id\":" + appId + ",\"title\":\"" + title + "\","
                + "\"game_id\":" + (gameId == null ? "null" : "\"" + gameId + "\"") + ","
                + "\"store\":{\"source\":\"steam\",\"id\":\"1\",\"name\":\"Steam\"},\"installed\":true,"
                + "\"running\":" + running + ",\"artwork\":{\"cover\":\"/api/native/v1/cover/" + installId
                + "\",\"version\":\"v1\"}}";
    }

    public static String library(String revision, String... games) {
        return "{\"revision\":\"" + revision + "\",\"games\":[" + String.join(",", games) + "]}";
    }

    public static String session(String state, String owner, String appJson, String actionsJson) {
        return "{\"revision\":7,\"state\":\"" + state + "\",\"owner\":\"" + owner + "\",\"app\":" + appJson
                + ",\"game_running\":true,\"since_unix_ms\":1000,\"actions\":" + actionsJson + "}";
    }

    public static final String IDLE = session("idle", "none", "null", "[]");

    public static String ok(String json) {
        return "{\"ok\":" + json + "}";
    }

    /** A host's {@code /wake} answer with these adapters and no broadcasts, as tests in other packages store it. */
    public static NativeWake wake(String... macs) throws org.json.JSONException {
        StringBuilder targets = new StringBuilder();
        for (String mac : macs) {
            targets.append(targets.length() == 0 ? "" : ",").append("{\"mac\":\"").append(mac)
                    .append("\",\"broadcasts\":[]}");
        }
        return NativeWake.parse(new org.json.JSONObject("{\"revision\":1,\"targets\":[" + targets + "]}"));
    }

    public static String error(String kind, String message) {
        return "{\"error\":{\"kind\":\"" + kind + "\",\"message\":\"" + message + "\"}}";
    }

    /** Answers by host and operation, and remembers every call. */
    public static final class FakeTransport implements NativeApiTransport {
        public final Map<String, String> replies = new HashMap<>();
        public final List<String> calls = Collections.synchronizedList(new ArrayList<>());
        public final List<String> arguments = Collections.synchronizedList(new ArrayList<>());

        public void reply(String hostUuid, String op, String envelope) {
            replies.put(hostUuid + "|" + op, envelope);
        }

        public void unreachable(String hostUuid) {
            replies.put(hostUuid + "|*", error("network", "connection refused"));
        }

        @Override
        public String request(HostEndpoint host, String op, String argument) throws NativeApiException {
            calls.add(host.hostUuid + "|" + op);
            arguments.add(argument);
            String reply = replies.get(host.hostUuid + "|" + op);
            if (reply == null) {
                reply = replies.get(host.hostUuid + "|*");
            }
            if (reply == null) {
                throw new NativeApiException(NativeApiException.Kind.UNAVAILABLE, "no scripted reply for " + op);
            }
            return reply;
        }
    }
}
