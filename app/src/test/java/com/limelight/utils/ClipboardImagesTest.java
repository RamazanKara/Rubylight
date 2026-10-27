package com.limelight.utils;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.ClipData;
import android.content.ClipDescription;
import android.content.Context;
import android.net.Uri;

import com.limelight.nvstream.http.ClipboardImageException;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.util.HashSet;
import java.util.Set;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ClipboardImagesTest {
    /** A PNG signature and IHDR chunk of the given size; enough for the header checks. */
    private static byte[] png(int width, int height) {
        ByteBuffer png = ByteBuffer.allocate(33);
        png.put(new byte[] {(byte) 0x89, 'P', 'N', 'G', '\r', '\n', 0x1a, '\n'});
        png.putInt(13).put(new byte[] {'I', 'H', 'D', 'R'});
        png.putInt(width).putInt(height);
        return png.array();
    }

    @Test
    public void aClipIsAnImageByItsDescriptionAndATextClipIsNone() {
        Context context = RuntimeEnvironment.getApplication();
        Uri image = Uri.parse("content://photos/1");
        ClipData clip = new ClipData(new ClipDescription("image", new String[] {"image/png"}), new ClipData.Item(image));
        assertEquals(image, ClipboardImages.imageUri(clip, context.getContentResolver()));
        assertNull(ClipboardImages.imageUri(ClipData.newPlainText("text", "hello"), context.getContentResolver()));
        assertNull(ClipboardImages.imageUri(null, context.getContentResolver()));
    }

    @Test
    public void copyingStopsAtTheLimitAsTooLarge() throws IOException {
        byte[] bytes = new byte[100];
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        assertEquals(100, ClipboardImages.copyAtMost(new ByteArrayInputStream(bytes), out, 100));
        assertArrayEquals(bytes, out.toByteArray());
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, assertThrows(ClipboardImageException.class,
                () -> ClipboardImages.copyAtMost(new ByteArrayInputStream(new byte[101]), new ByteArrayOutputStream(), 100)).reason);
    }

    @Test
    public void aBitmapThatWouldNotFitTheHeapIsRefusedAsTooLargeNotScaled() throws IOException {
        // 4000 x 3000 ARGB is 48 MB: decoded with 64 MB free, refused with 32 MB rather than sampled down.
        ClipboardImages.checkDecode(4000, 3000, 64L << 20);
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, assertThrows(ClipboardImageException.class,
                () -> ClipboardImages.checkDecode(4000, 3000, 32L << 20)).reason);
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, assertThrows(ClipboardImageException.class,
                () -> ClipboardImages.checkDecode(16385, 1, Long.MAX_VALUE)).reason);
        assertTrue(ClipboardImages.availableHeap() > 0);
    }

    @Test
    public void anOutgoingPngIsCopiedAsItIsAndOnlyThatCopyStays() throws IOException {
        Context context = RuntimeEnvironment.getApplication();
        File source = new File(context.getCacheDir(), "picked.png");
        byte[] png = png(8, 8);
        java.nio.file.Files.write(source.toPath(), png);
        File sent = ClipboardImages.readPng(context, Uri.fromFile(source));
        assertArrayEquals(png, java.nio.file.Files.readAllBytes(sent.toPath()));
        File[] outgoing = new File(context.getCacheDir(), ClipboardImages.OUTGOING).listFiles();
        assertEquals(1, outgoing.length);
        ClipboardImages.discard(sent);
        assertFalse(sent.exists());
        // A PNG over the host's limits is refused from its header, and the copy it made goes too. Converting other
        // images is left to the device: Robolectric's BitmapFactory decodes any bytes.
        java.nio.file.Files.write(source.toPath(), png(16385, 1));
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, assertThrows(ClipboardImageException.class,
                () -> ClipboardImages.readPng(context, Uri.fromFile(source))).reason);
        assertEquals(0, new File(context.getCacheDir(), ClipboardImages.OUTGOING).listFiles().length);
    }

    @Test
    public void eachRefusalHasItsOwnMessage() {
        Set<Integer> messages = new HashSet<>();
        for (ClipboardImageException.Reason reason : ClipboardImageException.Reason.values()) {
            messages.add(ClipboardImages.message(reason));
        }
        assertEquals(ClipboardImageException.Reason.values().length, messages.size());
    }

    @Test
    public void aReceivedImageReplacesTheOneBeforeOnlyOnceItIsKept() throws IOException {
        Context context = RuntimeEnvironment.getApplication();
        // FileProvider's URI is left to the device: it matches roots by '/', which a Windows test machine lacks.
        File first = ClipboardImages.newIncoming(context);
        File second = ClipboardImages.newIncoming(context);
        // Until the second is on the clipboard, the first is what the clipboard still points at.
        assertTrue(first.exists());
        ClipboardImages.keepOnly(context, second);
        File[] kept = new File(context.getCacheDir(), ClipboardImages.DIRECTORY).listFiles();
        assertEquals(1, kept.length);
        assertEquals(second, kept[0]);
    }
}
