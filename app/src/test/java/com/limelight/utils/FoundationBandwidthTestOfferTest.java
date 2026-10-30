package com.limelight.utils;

import android.app.Application;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvHTTP;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.io.IOException;

import static org.junit.Assert.*;

// The speed half of the connection test goes to a Sunshine-family PC only once it said it has
// Foundation Sunshine's test; stock Sunshine, Apollo and Vibepollo are asked once, then left alone.
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class FoundationBandwidthTestOfferTest {
    @Test
    public void eachPcIsAskedOnceAndAFailedQuestionIsAskedAgain() throws Exception {
        Http stock = new Http(false);
        assertFalse(PyroWaveBandwidthTest.hasFoundationTest(stock, "stock-sunshine"));
        assertFalse(PyroWaveBandwidthTest.hasFoundationTest(stock, "stock-sunshine"));
        assertEquals("asked once", 1, stock.questions);

        Http foundation = new Http(true);
        assertTrue(PyroWaveBandwidthTest.hasFoundationTest(foundation, "foundation"));
        assertTrue(PyroWaveBandwidthTest.hasFoundationTest(foundation, "foundation"));
        assertEquals(1, foundation.questions);

        Http offline = new Http(true);
        offline.fail = true;
        assertThrows(IOException.class, () -> PyroWaveBandwidthTest.hasFoundationTest(offline, "offline"));
        offline.fail = false;
        assertTrue("not cached after a failure", PyroWaveBandwidthTest.hasFoundationTest(offline, "offline"));
        assertEquals(2, offline.questions);
    }

    private static class Http extends NvHTTP {
        final boolean has;
        boolean fail;
        int questions;

        Http(boolean has) throws IOException {
            super(new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test", null,
                    new LimelightCryptoProvider() {
                        public java.security.cert.X509Certificate getClientCertificate() { return null; }
                        public java.security.PrivateKey getClientPrivateKey() { return null; }
                        public byte[] getPemEncodedClientCertificate() { return new byte[0]; }
                        public String encodeBase64String(byte[] data) { return ""; }
                    });
            this.has = has;
        }

        @Override public boolean supportsFoundationBandwidthTest() throws IOException {
            questions++;
            if (fail) throw new IOException("no route");
            return has;
        }
    }
}
