package com.limelight.utils;

import android.app.Application;
import android.content.Context;

import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33, application = Application.class)
public class ClipboardSyncTest {
    private Context context;
    private final FakeBridge bridge = new FakeBridge();
    private final FakeClipboard clipboard = new FakeClipboard();
    private final List<String> messages = new ArrayList<>();
    private ClipboardSync sync;

    // The Rust engine as far as Java sees it: what it was handed and what it answers.
    private static final class FakeBridge implements ClipboardSync.Bridge {
        int startResult = MoonBridge.CLIPBOARD_SYNC_ON;
        int localResult = MoonBridge.CLIPBOARD_COPY_SENT;
        final List<String> started = new ArrayList<>();
        final List<String> sent = new ArrayList<>();

        @Override public int start(String current) {
            started.add(current);
            return startResult;
        }
        @Override public int local(String text) {
            sent.add(text);
            return localResult;
        }
    }

    // Android's clipboard: a refused read throws, like ClipboardManager on some devices.
    private static final class FakeClipboard implements ClipboardSync.Clipboard {
        String text = "before";
        boolean refuse;
        boolean failWrite;
        Runnable listener;
        final List<String> written = new ArrayList<>();

        @Override public String read() {
            if (refuse) throw new SecurityException("not focused");
            return text;
        }
        @Override public boolean write(String value) {
            if (failWrite) return false;
            written.add(value);
            text = value;
            return true;
        }
        @Override public void listen(Runnable changed) { listener = changed; }
        @Override public void stopListening() { listener = null; }
    }

    @Before
    public void setUp() {
        context = RuntimeEnvironment.getApplication();
        sync = new ClipboardSync(context, bridge, clipboard, messages::add);
    }

    private String text(int id, Object... args) {
        return context.getString(id, args);
    }

    @Test
    public void itStartsWithFocusAndKeepsTheClipboardHere() {
        sync.connectionStarted(true);
        assertEquals(Collections.singletonList("before"), bridge.started);
        assertNotNull(clipboard.listener);
        clipboard.text = "copied";
        clipboard.listener.run();
        assertEquals(Collections.singletonList("copied"), bridge.sent);
        assertTrue(messages.isEmpty());
    }

    // Only a focused app may read the clipboard: the start waits for focus.
    @Test
    public void withoutFocusItStartsOnceFocusComes() {
        sync.connectionStarted(false);
        assertTrue(bridge.started.isEmpty());
        sync.focusChanged(true);
        assertEquals(Collections.singletonList("before"), bridge.started);
        assertTrue("the clipboard at the start is not sent", bridge.sent.isEmpty());
    }

    // Android tells of changes only while focused; a copy made in another app goes when focus is back.
    @Test
    public void aCopyMadeElsewhereGoesWhenFocusComesBack() {
        sync.connectionStarted(true);
        sync.focusChanged(false);
        clipboard.text = "from another app";
        clipboard.listener.run();
        assertTrue(bridge.sent.isEmpty());
        sync.focusChanged(true);
        assertEquals(Collections.singletonList("from another app"), bridge.sent);
    }

    @Test
    public void aRefusedReadIsSaidOnce() {
        sync.connectionStarted(true);
        clipboard.refuse = true;
        clipboard.listener.run();
        sync.focusChanged(true);
        assertEquals(Collections.singletonList(text(R.string.clipboard_sync_read_refused)), messages);
        assertTrue(bridge.sent.isEmpty());
    }

    // Android answers an app it does not let read with no clip, even right after a change notice.
    @Test
    public void noClipAfterAChangeNoticeIsARefusedRead() {
        sync.connectionStarted(true);
        clipboard.text = null;
        sync.focusChanged(true);
        assertTrue("no clip on a focus check is an empty clipboard", messages.isEmpty());
        clipboard.listener.run();
        assertEquals(Collections.singletonList(text(R.string.clipboard_sync_read_refused)), messages);
    }

    @Test
    public void aRefusedReadAtTheStartStillStartsWithoutAClipboard() {
        clipboard.refuse = true;
        sync.connectionStarted(true);
        assertEquals(Collections.singletonList((String) null), bridge.started);
        assertNotNull(clipboard.listener);
    }

    @Test
    public void thePcsCopyLandsHereAndAFailedWriteIsSaidOnce() {
        sync.connectionStarted(true);
        sync.received("from the pc");
        assertEquals(Collections.singletonList("from the pc"), clipboard.written);
        // Android reports the write as a change; the engine's rules skip it.
        bridge.localResult = MoonBridge.CLIPBOARD_COPY_SKIPPED;
        clipboard.listener.run();
        assertTrue(messages.isEmpty());
        clipboard.failWrite = true;
        sync.received("a");
        sync.received("b");
        assertEquals(Collections.singletonList(text(R.string.clipboard_sync_write_failed)), messages);
    }

    @Test
    public void troubleNamesTheMenuEntryThatCarriesIt() {
        sync.connectionStarted(true);
        bridge.localResult = MoonBridge.CLIPBOARD_COPY_TOO_LARGE;
        clipboard.text = "large";
        clipboard.listener.run();
        clipboard.listener.run();
        sync.refused(MoonBridge.CLIPBOARD_REFUSED_IMAGE);
        sync.refused(MoonBridge.CLIPBOARD_REFUSED_IMAGE);
        sync.refused(MoonBridge.CLIPBOARD_REFUSED_OTHER);
        bridge.localResult = MoonBridge.CLIPBOARD_COPY_FAILED;
        clipboard.listener.run();
        assertEquals(Arrays.asList(
                text(R.string.clipboard_sync_too_large, text(R.string.stream_clipboard_send)),
                text(R.string.clipboard_sync_image, text(R.string.stream_clipboard_image_receive)),
                text(R.string.clipboard_sync_refused),
                text(R.string.clipboard_sync_send_failed)), messages);
    }

    @Test
    public void aPcThatDidNotOfferItIsSaid() {
        bridge.startResult = MoonBridge.CLIPBOARD_SYNC_NOT_OFFERED;
        sync.connectionStarted(true);
        assertEquals(Collections.singletonList(text(R.string.clipboard_sync_not_offered)), messages);
        assertNull(clipboard.listener);
        sync.received("ignored");
        sync.focusChanged(true);
        assertTrue(clipboard.written.isEmpty());
        assertTrue(bridge.sent.isEmpty());

        bridge.startResult = MoonBridge.CLIPBOARD_SYNC_FAILED;
        new ClipboardSync(context, bridge, clipboard, messages::add).connectionStarted(true);
        assertEquals(text(R.string.clipboard_sync_failed), messages.get(messages.size() - 1));
    }

    @Test
    public void stopEndsItAndStopsListening() {
        sync.connectionStarted(true);
        sync.stop();
        assertNull(clipboard.listener);
        sync.received("late");
        sync.focusChanged(true);
        assertTrue(clipboard.written.isEmpty());
        assertTrue(bridge.sent.isEmpty());
    }
}
