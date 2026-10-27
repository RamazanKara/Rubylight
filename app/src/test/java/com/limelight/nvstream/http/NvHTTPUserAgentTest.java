package com.limelight.nvstream.http;

import android.app.Application;

import com.limelight.BuildConfig;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.lang.reflect.Field;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;

import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509TrustManager;

import okhttp3.Interceptor;
import okhttp3.MediaType;
import okhttp3.OkHttpClient;
import okhttp3.Protocol;
import okhttp3.Request;
import okhttp3.Response;
import okhttp3.ResponseBody;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NvHTTPUserAgentTest {
    private static final String EMPTY = "{\"version\":1,\"sampled_at_unix_ms\":1791712800000," +
            "\"interval_ms\":1000,\"cpu\":{},\"gpus\":[]}";

    @Test
    public void theUserAgentNamesTheAppVersion() {
        assertEquals("Nightjar/" + BuildConfig.VERSION_NAME, NvHTTP.USER_AGENT);
        assertTrue(NvHTTP.USER_AGENT, NvHTTP.USER_AGENT.matches("Nightjar/\\d+\\.\\d+\\.\\d+\\S*"));
    }

    @Test
    public void everyClientToTheHostSendsTheUserAgent() throws Exception {
        NvHTTP http = http();
        for (String name : new String[] {"httpClientLongConnectTimeout",
                "httpClientLongConnectNoReadTimeout", "httpClientShortConnectTimeout"}) {
            List<String> agents = new ArrayList<>();
            OkHttpClient client = capture(client(http, name), agents);
            client.newCall(new Request.Builder().url("http://192.0.2.1:47989/serverinfo").build())
                    .execute().close();
            assertEquals(name, List.of(NvHTTP.USER_AGENT), agents);
        }
    }

    @Test
    public void aCallThatBuildsItsOwnClientKeepsTheUserAgent() throws Exception {
        NvHTTP http = http();
        List<String> agents = new ArrayList<>();
        Field field = NvHTTP.class.getDeclaredField("httpClientShortConnectTimeout");
        field.setAccessible(true);
        field.set(http, capture((OkHttpClient) field.get(http), agents));
        http.getHardwareMetrics();
        assertEquals(List.of(NvHTTP.USER_AGENT), agents);
    }

    /** The client with a last interceptor that records the User-Agent and answers for the host. */
    private static OkHttpClient capture(OkHttpClient client, List<String> agents) {
        return client.newBuilder().addInterceptor(chain -> {
            agents.add(chain.request().header("User-Agent"));
            return response(chain);
        }).build();
    }

    private static Response response(Interceptor.Chain chain) {
        return new Response.Builder().request(chain.request()).protocol(Protocol.HTTP_1_1).code(200)
                .message("Fixture").body(ResponseBody.create(EMPTY, MediaType.get("application/json"))).build();
    }

    private static OkHttpClient client(NvHTTP http, String name) throws Exception {
        Field field = NvHTTP.class.getDeclaredField(name);
        field.setAccessible(true);
        return (OkHttpClient) field.get(http);
    }

    private static NvHTTP http() throws Exception {
        NvHTTP http = new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984,
                "device", null, new LimelightCryptoProvider() {
            public X509Certificate getClientCertificate() { return null; }
            public PrivateKey getClientPrivateKey() { return null; }
            public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            public String encodeBase64String(byte[] data) { return ""; }
        });
        TrustManagerFactory factory = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
        factory.init((KeyStore) null);
        for (TrustManager manager : factory.getTrustManagers()) {
            if (manager instanceof X509TrustManager) {
                http.setServerCert(((X509TrustManager) manager).getAcceptedIssuers()[0]);
                break;
            }
        }
        return http;
    }
}
