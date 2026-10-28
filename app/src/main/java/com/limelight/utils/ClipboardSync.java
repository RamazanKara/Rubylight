package com.limelight.utils;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.os.Build;
import android.os.PersistableBundle;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.jni.MoonBridge;

import java.util.function.Consumer;

/**
 * Clipboard sync (0x5508) for one stream with a Rubylight PC that offered it: text copied on this device goes on
 * the PC's clipboard, and the PC's copies land here. The Rust engine keeps the rules (the clipboard as it was at
 * the start stays here, the PC's copy written here does not go back, the PC's echo of a copy is dropped).
 * Android 10 and later let an app read the clipboard only while it has focus and tell it of changes only then, so
 * this reads on a change and again each time focus comes back; a read Android refused is said once. Images go by
 * the stream menu's image entries. Main thread only.
 */
public final class ClipboardSync {
    /** The launch query parameter that asks a Rubylight PC for it. */
    public static final String LAUNCH_PARAMETER = "rlClipboardSync";

    /** The Rust engine; MoonBridge outside tests. */
    public interface Bridge {
        int start(String current);
        int local(String text);
    }

    /** This device's clipboard; ClipboardManager outside tests. */
    public interface Clipboard {
        /**
         * The text on it, "" when the clip holds none, null without a clip (as Android also answers an app it does
         * not let read); throws SecurityException when Android refused outright.
         */
        String read();
        /** false when the write failed. */
        boolean write(String text);
        void listen(Runnable changed);
        void stopListening();
    }

    private static final int TOLD_READ = 1;
    private static final int TOLD_LARGE = 2;
    private static final int TOLD_SEND = 4;
    private static final int TOLD_WRITE = 8;
    private static final int TOLD_IMAGE = 16;
    private static final int TOLD_REFUSED = 32;

    private final Context context;
    private final Bridge bridge;
    private final Clipboard clipboard;
    private final Consumer<String> say;
    private boolean focused;
    private boolean waiting;
    private boolean running;
    private int told;

    public ClipboardSync(Context context, Bridge bridge, Clipboard clipboard, Consumer<String> say) {
        this.context = context;
        this.bridge = bridge;
        this.clipboard = clipboard;
        this.say = say;
    }

    /** For the stream: the Rust engine and this device's clipboard. */
    public static ClipboardSync forStream(Context context, Consumer<String> say) {
        Bridge bridge = new Bridge() {
            @Override public int start(String current) { return MoonBridge.clipboardSyncStart(current); }
            @Override public int local(String text) { return MoonBridge.clipboardSyncLocal(text); }
        };
        return new ClipboardSync(context, bridge, new AndroidClipboard(context), say);
    }

    /**
     * The connection started. Starts now with focus, else once focus comes: the clipboard here as it was must be
     * read to keep it here, and only a focused app may read it.
     */
    public void connectionStarted(boolean hasFocus) {
        focused = hasFocus;
        if (focused) start();
        else waiting = true;
    }

    private void start() {
        waiting = false;
        String current = null;
        try {
            current = clipboard.read();
        } catch (SecurityException e) {
            LimeLog.warning("Clipboard sync: Android refused the read at the start: " + e.getMessage());
        }
        int result = bridge.start(current);
        if (result == MoonBridge.CLIPBOARD_SYNC_ON) {
            running = true;
            clipboard.listen(this::changed);
            LimeLog.info("Clipboard sync: on");
        } else if (result == MoonBridge.CLIPBOARD_SYNC_NOT_OFFERED) {
            LimeLog.warning("Clipboard sync: the PC did not offer it");
            say.accept(context.getString(R.string.clipboard_sync_not_offered));
        } else {
            LimeLog.warning("Clipboard sync: could not start (" + result + ")");
            say.accept(context.getString(R.string.clipboard_sync_failed));
        }
    }

    /** The window gained or lost focus: a copy made in another app meanwhile goes now. */
    public void focusChanged(boolean hasFocus) {
        focused = hasFocus;
        if (!hasFocus) return;
        if (waiting) start();
        else if (running) send(false);
    }

    private void changed() {
        if (running && focused) send(true);
    }

    /** Reads the clipboard and sends it unless the rules know it. A change notice means a clip is there. */
    private void send(boolean noticed) {
        String text;
        try {
            text = clipboard.read();
        } catch (SecurityException e) {
            LimeLog.warning("Clipboard sync: Android refused the read: " + e.getMessage());
            tell(TOLD_READ, context.getString(R.string.clipboard_sync_read_refused));
            return;
        }
        if (text == null) {
            if (noticed) {
                LimeLog.warning("Clipboard sync: Android gave no clip after a change notice");
                tell(TOLD_READ, context.getString(R.string.clipboard_sync_read_refused));
            }
            return;
        }
        if (text.isEmpty()) return;
        int result = bridge.local(text);
        if (result == MoonBridge.CLIPBOARD_COPY_TOO_LARGE) {
            tell(TOLD_LARGE, context.getString(R.string.clipboard_sync_too_large,
                    context.getString(R.string.stream_clipboard_send)));
        } else if (result == MoonBridge.CLIPBOARD_COPY_FAILED) {
            tell(TOLD_SEND, context.getString(R.string.clipboard_sync_send_failed));
        }
    }

    /** A copy from the PC. Android tells of the write as a change, which the rules then skip. */
    public void received(String text) {
        if (!running) return;
        if (!clipboard.write(text)) {
            tell(TOLD_WRITE, context.getString(R.string.clipboard_sync_write_failed));
        }
    }

    /** A copy from the PC the rules refused: MoonBridge.CLIPBOARD_REFUSED_*. */
    public void refused(int reason) {
        if (!running) return;
        if (reason == MoonBridge.CLIPBOARD_REFUSED_IMAGE) {
            tell(TOLD_IMAGE, context.getString(R.string.clipboard_sync_image,
                    context.getString(R.string.stream_clipboard_image_receive)));
        } else {
            tell(TOLD_REFUSED, context.getString(R.string.clipboard_sync_refused));
        }
    }

    public void stop() {
        if (running) clipboard.stopListening();
        running = false;
        waiting = false;
    }

    private void tell(int kind, String text) {
        if ((told & kind) != 0) return;
        told |= kind;
        say.accept(text);
    }

    /** ClipboardManager; a write is marked sensitive like the stream menu's, so Android shows no preview. */
    static final class AndroidClipboard implements Clipboard {
        private final ClipboardManager manager;
        private ClipboardManager.OnPrimaryClipChangedListener listener;

        AndroidClipboard(Context context) {
            manager = (ClipboardManager) context.getSystemService(Context.CLIPBOARD_SERVICE);
        }

        @Override
        public String read() {
            ClipData clip = manager.getPrimaryClip();
            if (clip == null) return null;
            if (clip.getItemCount() == 0 || clip.getItemAt(0).getText() == null) return "";
            return clip.getItemAt(0).getText().toString();
        }

        @Override
        public boolean write(String text) {
            ClipData clip = ClipData.newPlainText(BuildConfig.BRAND_NAME, text);
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                PersistableBundle extras = new PersistableBundle();
                extras.putBoolean("android.content.extra.IS_SENSITIVE", true);
                clip.getDescription().setExtras(extras);
            }
            try {
                manager.setPrimaryClip(clip);
                return true;
            } catch (RuntimeException e) {
                LimeLog.warning("Clipboard sync: could not write the clipboard: " + e);
                return false;
            }
        }

        @Override
        public void listen(Runnable changed) {
            stopListening();
            listener = changed::run;
            manager.addPrimaryClipChangedListener(listener);
        }

        @Override
        public void stopListening() {
            if (listener != null) manager.removePrimaryClipChangedListener(listener);
            listener = null;
        }
    }
}
