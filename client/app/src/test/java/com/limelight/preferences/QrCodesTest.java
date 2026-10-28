package com.limelight.preferences;

import com.google.zxing.BarcodeFormat;
import com.google.zxing.EncodeHintType;
import com.google.zxing.common.BitMatrix;
import com.google.zxing.qrcode.QRCodeWriter;

import org.junit.Test;

import java.util.Collections;

import static org.junit.Assert.*;

public class QrCodesTest {
    private static final String LINK = "rubylight://pair?v=1&host=192.168.4.2&port=47989&otp=4821&pass=a%26b%3Dc%20d"
            + "&id=0123ABCD&fp=" + "ab".repeat(32) + "&name=Wohnzimmer-PC%20%C3%A4";

    /** A camera-like luminance plane: padded rows, and a last row that ends at the picture's width. */
    private static byte[] plane(BitMatrix code, int width, int height, int rowStride) {
        byte[] plane = new byte[rowStride * (height - 1) + width];
        java.util.Arrays.fill(plane, (byte) 0xF0);
        int left = (width - code.getWidth()) / 2, top = (height - code.getHeight()) / 2;
        for (int y = 0; y < code.getHeight(); y++) {
            for (int x = 0; x < code.getWidth(); x++) {
                if (code.get(x, y)) plane[(top + y) * rowStride + left + x] = 0x10;
            }
        }
        return plane;
    }

    @Test
    public void readsThePairingLinkFromACameraFrame() throws Exception {
        BitMatrix code = new QRCodeWriter().encode(LINK, BarcodeFormat.QR_CODE, 300, 300,
                Collections.singletonMap(EncodeHintType.CHARACTER_SET, "UTF-8"));
        assertEquals(LINK, QrCodes.decode(plane(code, 640, 480, 704), 640, 480, 704));
    }

    @Test
    public void framesWithoutACodeReadNothing() {
        byte[] blank = new byte[640 * 480];
        assertNull(QrCodes.decode(blank, 640, 480, 640));
        assertNull(QrCodes.decode(blank, 640, 480, 320));
        assertNull(QrCodes.decode(blank, 0, 480, 640));
    }
}
