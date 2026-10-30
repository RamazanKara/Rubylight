package com.limelight.nvstream;

import android.app.Application;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvHTTP;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.IOException;
import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

// Robolectric for org.json, which the request bodies use.
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NvConnectionFoundationAbrTest {
    @Test
    public void thePcsControllerGoesOffAfterTheStreamStopped() throws Exception {
        NvConnection connection = new NvConnection(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989),
                47984, "test", new StreamConfiguration.Builder().build(), null, null);
        Http commands = new Http();
        Http abr = new Http();
        field(connection, "http", commands);
        field(connection, "foundationAbrHttp", abr);

        assertEquals(30000, connection.startFoundationAbr(50000));
        // stop() cancels the stream's requests; a start that was in flight, or the stream's end,
        // still turns the PC's controller off.
        connection.stop();
        connection.stopFoundationAbr();

        assertEquals(1, commands.cancellations);
        assertEquals(0, abr.cancellations);
        assertEquals(0, commands.requests.size());
        assertEquals(2, abr.requests.size());
        assertTrue(abr.requests.get(0).getBoolean("enabled"));
        assertFalse(abr.requests.get(1).getBoolean("enabled"));
    }

    private static void field(NvConnection connection, String name, Object value) throws Exception {
        Field field = NvConnection.class.getDeclaredField(name);
        field.setAccessible(true);
        field.set(connection, value);
    }

    private static class Http extends NvHTTP {
        final List<JSONObject> requests = new ArrayList<>();
        int cancellations;

        Http() throws IOException {
            super(new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test", null,
                    new LimelightCryptoProvider() {
                        public java.security.cert.X509Certificate getClientCertificate() { return null; }
                        public java.security.PrivateKey getClientPrivateKey() { return null; }
                        public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
                        public String encodeBase64String(byte[] data) { return ""; }
                    });
        }

        @Override public boolean supportsFoundationAbr() throws IOException {
            if (cancellations > 0) throw new IOException("cancelled");
            return true;
        }

        @Override public int configureFoundationAbr(JSONObject request) throws IOException {
            if (cancellations > 0) throw new IOException("cancelled");
            requests.add(request);
            return 30000;
        }

        @Override public void cancelPendingRequests() { cancellations++; }
    }
}
