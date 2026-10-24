package com.limelight.ui;

import android.app.Application;
import android.content.Context;
import android.widget.Toast;

import com.limelight.computers.HostNotices;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowToast;

import java.time.Duration;
import java.util.Arrays;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class StreamMessagesTest {
    private final Context context = RuntimeEnvironment.getApplication();

    // What a stream start used to queue as one toast after another.
    private static void start(StreamMessages messages) {
        messages.notice("pc", HostNotices.Notice.HDR_UNAVAILABLE, "This screen can’t show HDR, so the stream is in SDR.");
        messages.notice("pc", HostNotices.Notice.CODEC_UNAVAILABLE, "This device can’t play AV1, so the stream uses another codec.");
        messages.note("External display: frames are shown as soon as they arrive.");
        messages.countedNotice("This PC has no virtual display, so it streams its own screen.");
        messages.notice("pc", HostNotices.Notice.AUTOMATIC_QUALITY_UNAVAILABLE, "Automatic quality is off.");
    }

    @Test
    public void aStreamStartShowsOneMessageAndTheRestWaitInTheStatusLine() {
        StreamMessages messages = new StreamMessages(context);
        start(messages);
        assertEquals("One message instead of a queue", 1, ShadowToast.shownToastCount());
        assertEquals("This screen can’t show HDR, so the stream is in SDR.", ShadowToast.getTextOfLatestToast());
        assertEquals(Arrays.asList("This screen can’t show HDR, so the stream is in SDR.",
                "This device can’t play AV1, so the stream uses another codec.",
                "External display: frames are shown as soon as they arrive.",
                "This PC has no virtual display, so it streams its own screen.",
                "Automatic quality is off."), messages.notes());
        // Facts that did not show are not marked for the PC: the next streams say them, one each.
        assertFalse(HostNotices.claim(context, "pc", HostNotices.Notice.HDR_UNAVAILABLE));

        StreamMessages next = new StreamMessages(context);
        start(next);
        assertEquals(2, ShadowToast.shownToastCount());
        assertEquals("This device can’t play AV1, so the stream uses another codec.", ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void somethingToActOnReplacesAFactAndShowsOnce() {
        StreamMessages messages = new StreamMessages(context);
        messages.notice("pc", HostNotices.Notice.HDR_UNAVAILABLE, "No HDR.");
        Toast fact = ShadowToast.getLatestToast();
        messages.alert("The PC blocks input from this device.", Toast.LENGTH_LONG);
        assertTrue("The fact makes room", shadowOf(fact).isCancelled());
        assertEquals("The PC blocks input from this device.", ShadowToast.getTextOfLatestToast());
        messages.alert("The PC blocks input from this device.", Toast.LENGTH_LONG);
        assertEquals("The same message does not repeat while it shows", 2, ShadowToast.shownToastCount());
        // While it shows, a new fact waits for a later stream; afterwards one may show.
        messages.notice("pc", HostNotices.Notice.CODEC_UNAVAILABLE, "No AV1.");
        assertEquals(2, ShadowToast.shownToastCount());
        shadowOf(android.os.Looper.getMainLooper()).idleFor(Duration.ofMillis(StreamMessages.LONG_MS + 1));
        assertFalse(messages.showing());
        messages.notice("pc", HostNotices.Notice.CODEC_UNAVAILABLE, "No AV1.");
        assertEquals(3, ShadowToast.shownToastCount());
        messages.alert("The PC blocks input from this device.", Toast.LENGTH_SHORT);
        assertEquals("After its time a message may show again", 4, ShadowToast.shownToastCount());
    }

    @Test
    public void factsAppearOnceInTheStatusLine() {
        StreamMessages messages = new StreamMessages(context);
        messages.note("A");
        messages.note("A");
        messages.note("");
        messages.note(null);
        assertEquals(Arrays.asList("A"), messages.notes());
    }

    @Test
    public void aKeyedStatusIsReplacedByItsKeyAndRemovedWhenItClears() {
        StreamMessages messages = new StreamMessages(context);
        messages.note("External display: frames are shown as soon as they arrive.");
        assertTrue(messages.status("refresh", "Screen stays at 60 Hz."));
        assertFalse("The same text is not a change", messages.status("refresh", "Screen stays at 60 Hz."));
        assertTrue(messages.status("hdr", "HDR was asked for, but the PC reports SDR."));
        assertEquals(Arrays.asList("External display: frames are shown as soon as they arrive.",
                "Screen stays at 60 Hz.", "HDR was asked for, but the PC reports SDR."), messages.notes());

        assertTrue(messages.status("refresh", "Screen stays at 90 Hz."));
        assertEquals(Arrays.asList("External display: frames are shown as soon as they arrive.",
                "Screen stays at 90 Hz.", "HDR was asked for, but the PC reports SDR."), messages.notes());

        assertTrue(messages.status("refresh", null));
        assertFalse("Clearing what is not shown is no change", messages.status("refresh", ""));
        assertTrue(messages.status("hdr", ""));
        assertEquals(Arrays.asList("External display: frames are shown as soon as they arrive."), messages.notes());
    }
}
