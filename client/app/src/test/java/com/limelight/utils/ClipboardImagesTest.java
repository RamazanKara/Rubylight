package com.limelight.utils;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertThrows;

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
import java.io.File;
import java.io.IOException;
import java.util.HashSet;
import java.util.Set;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ClipboardImagesTest {
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
    public void readingStopsAtTheLimitAsTooLarge() throws IOException {
        byte[] bytes = new byte[100];
        assertArrayEquals(bytes, ClipboardImages.readAtMost(new ByteArrayInputStream(bytes), 100));
        assertEquals(ClipboardImageException.Reason.TOO_LARGE, assertThrows(ClipboardImageException.class,
                () -> ClipboardImages.readAtMost(new ByteArrayInputStream(new byte[101]), 100)).reason);
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
    public void aReceivedImageReplacesTheOneBefore() throws IOException {
        Context context = RuntimeEnvironment.getApplication();
        // FileProvider's URI is left to the device: it matches roots by '/', which a Windows test machine lacks.
        ClipboardImages.write(context, new byte[] {1});
        File second = ClipboardImages.write(context, new byte[] {2});
        File[] kept = new File(context.getCacheDir(), ClipboardImages.DIRECTORY).listFiles();
        assertEquals(1, kept.length);
        assertEquals(second, kept[0]);
        assertArrayEquals(new byte[] {2}, java.nio.file.Files.readAllBytes(second.toPath()));
    }
}
