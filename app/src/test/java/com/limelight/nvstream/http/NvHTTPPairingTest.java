package com.limelight.nvstream.http;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowBuild;
import org.robolectric.util.ReflectionHelpers;

import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;

import okhttp3.HttpUrl;
import okhttp3.MediaType;
import okhttp3.OkHttpClient;
import okhttp3.Protocol;
import okhttp3.Response;
import okhttp3.ResponseBody;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33, application = Application.class)
public class NvHTTPPairingTest {
    @Test
    public void pairingRequestsPreserveDeviceNamesWithoutAddingQueryParameters() throws Exception {
        NvHTTP http = new NvHTTP(new ComputerDetails.AddressTuple("192.0.2.1", 47989),
                47984, "client", null, new LimelightCryptoProvider() {
            public X509Certificate getClientCertificate() { return null; }
            public PrivateKey getClientPrivateKey() { return null; }
            public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
            public String encodeBase64String(byte[] data) { return ""; }
        });
        List<HttpUrl> requests = new ArrayList<>();
        OkHttpClient client = new OkHttpClient.Builder().addInterceptor(chain -> {
            requests.add(chain.request().url());
            return new Response.Builder().request(chain.request()).protocol(Protocol.HTTP_1_1)
                    .code(200).message("OK").body(ResponseBody.create(
                            "<root status_code=\"200\"><paired>1</paired></root>",
                            MediaType.get("application/xml"))).build();
        }).build();
        ReflectionHelpers.setField(http, "httpClientLongConnectTimeout", client);
        ReflectionHelpers.setField(http, "httpClientLongConnectNoReadTimeout", client);

        for (String model : new String[] {"Odin2 Portal", "SM-F956B", "München's handheld",
                "Model &uniqueid=other+50%#"}) {
            ShadowBuild.setModel(model);
            requests.clear();
            http.executePairingCommand("phrase=getservercert&salt=0011&clientcert=AABB", false);
            http.executePairingCommand("phrase=getservercert&salt=0011&clientcert=AABB&otpauth=CCDD", true);
            http.executePairingChallenge();

            assertEquals(3, requests.size());
            for (HttpUrl request : requests) {
                assertEquals("/pair", request.encodedPath());
                assertEquals(List.of(model), request.queryParameterValues("devicename"));
                assertEquals(List.of("client"), request.queryParameterValues("uniqueid"));
                assertEquals("1", request.queryParameter("updateState"));
                assertNotNull(request.queryParameter("uuid"));
            }
            for (HttpUrl request : requests.subList(0, 2)) {
                assertEquals("http", request.scheme());
                assertEquals("getservercert", request.queryParameter("phrase"));
                assertEquals("0011", request.queryParameter("salt"));
                assertEquals("AABB", request.queryParameter("clientcert"));
            }
            assertNull(requests.get(0).queryParameter("otpauth"));
            assertEquals("CCDD", requests.get(1).queryParameter("otpauth"));
            assertEquals("https", requests.get(2).scheme());
            assertEquals("pairchallenge", requests.get(2).queryParameter("phrase"));
        }
    }
}
