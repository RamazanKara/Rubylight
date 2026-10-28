package com.limelight.preferences;

import com.google.zxing.BarcodeFormat;
import com.google.zxing.BinaryBitmap;
import com.google.zxing.DecodeHintType;
import com.google.zxing.PlanarYUVLuminanceSource;
import com.google.zxing.ReaderException;
import com.google.zxing.common.HybridBinarizer;
import com.google.zxing.qrcode.QRCodeReader;

import java.util.Collections;
import java.util.EnumMap;
import java.util.Map;

/** Reads a QR code from a camera frame's luminance plane, with ZXing. */
final class QrCodes {
    private static final Map<DecodeHintType, Object> HINTS = new EnumMap<>(DecodeHintType.class);
    static {
        HINTS.put(DecodeHintType.POSSIBLE_FORMATS, Collections.singletonList(BarcodeFormat.QR_CODE));
        HINTS.put(DecodeHintType.TRY_HARDER, Boolean.TRUE);
        HINTS.put(DecodeHintType.CHARACTER_SET, "UTF-8");
    }

    private QrCodes() {
    }

    /**
     * The text of the QR code in the frame, or null when none is found. {@code luminance} holds {@code height}
     * rows of {@code rowStride} bytes, of which the first {@code width} are the picture; a last row cut short, as
     * camera planes end, is padded.
     */
    static String decode(byte[] luminance, int width, int height, int rowStride) {
        if (width <= 0 || height <= 0 || rowStride < width) {
            return null;
        }
        byte[] rows = luminance;
        if (luminance.length < rowStride * height) {
            rows = new byte[rowStride * height];
            System.arraycopy(luminance, 0, rows, 0, luminance.length);
        }
        PlanarYUVLuminanceSource source = new PlanarYUVLuminanceSource(rows, rowStride, height, 0, 0, width, height,
                false);
        try {
            return new QRCodeReader().decode(new BinaryBitmap(new HybridBinarizer(source)), HINTS).getText();
        } catch (ReaderException e) {
            return null;
        }
    }
}
