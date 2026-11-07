package com.limelight.ui;

import android.app.Activity;
import android.app.Dialog;
import android.graphics.Bitmap;
import android.graphics.drawable.ColorDrawable;
import android.os.Build;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowManager;
import android.widget.ImageView;
import android.widget.TextView;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;

/**
 * Connecting, launching and a failed connection, in one window of its own over the stream's still empty surface: the
 * cover in its ambient, the moon's orbit, one word. It goes away (the window is removed, as the spinner's was) the moment
 * the connection stands, so it adds nothing to the stream; nothing waits for it.
 */
public final class ConnectingScreen {
    /** What a failed connection offers; each runs on the main thread. */
    public interface Failure {
        void onTryAgain();

        /** Tests the network on a worker and returns the words for the result. */
        CharSequence testNetwork();

        void onBack();

        /** The stream's size, rate and bitrate for the row that opens its settings; null shows no such row. */
        default CharSequence streamSettings() {
            return null;
        }

        /** The row was pressed: this start has ended, and the settings take over. */
        default void onStreamSettings() {
        }
    }

    private static final long CANCEL_FOCUS_MS = 600;

    private final Activity activity;
    private final Dialog dialog;
    private final String title;
    private final BackdropView backdrop;
    private final OrbitView orbit;
    private final ImageView coverImage;
    private final NoArtTile coverTile;
    private final View coverCard;
    private final TextView word;
    private final TextView line;
    private final View cancel;
    private final View stage;
    private final View panel;
    private boolean failed;
    private volatile boolean dismissed;

    private ConnectingScreen(Activity activity, String title, String pc, Runnable onCancel) {
        this.activity = activity;
        this.title = title == null ? "" : title;
        dialog = new Dialog(activity, R.style.ConnectingScreen);
        // The app's own face: this is not over a stream yet.
        AppTheme.applyOverlays(activity, dialog.getContext().getTheme());
        Window window = dialog.getWindow();
        dialog.setContentView(R.layout.connecting_screen);
        if (window != null) {
            window.setLayout(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT);
            int canvas = MaterialColors.getColor(activity, R.attr.glassCanvas, 0xFF0B0C10);
            window.setBackgroundDrawable(new ColorDrawable(canvas));
            window.addFlags(WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS | WindowManager.LayoutParams.FLAG_FULLSCREEN);
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                window.getAttributes().layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            }
            // The stream hides the bars; this window must not bring them back.
            window.getDecorView().setSystemUiVisibility(activity.getWindow().getDecorView().getSystemUiVisibility());
        }
        GlassHost host = dialog.findViewById(R.id.conn_host);
        backdrop = dialog.findViewById(R.id.conn_backdrop);
        orbit = dialog.findViewById(R.id.conn_orbit);
        coverCard = dialog.findViewById(R.id.conn_cover);
        coverImage = dialog.findViewById(R.id.conn_cover_image);
        coverTile = dialog.findViewById(R.id.conn_cover_tile);
        word = dialog.findViewById(R.id.conn_word);
        line = dialog.findViewById(R.id.conn_line);
        cancel = dialog.findViewById(R.id.conn_cancel);
        stage = dialog.findViewById(R.id.conn_stage);
        panel = dialog.findViewById(R.id.conn_panel);
        ((TextView) dialog.findViewById(R.id.conn_title)).setText(this.title);
        TextView pcName = dialog.findViewById(R.id.conn_pc);
        pcName.setText(pc);
        pcName.setVisibility(pc == null || pc.isEmpty() ? View.GONE : View.VISIBLE);
        coverTile.setText(this.title);
        coverCard.setContentDescription(this.title);

        int canvas = MaterialColors.getColor(activity, R.attr.glassCanvas, 0xFF0B0C10);
        backdrop.setCanvasColor(canvas);
        backdrop.setBake(false, activity.getResources().getConfiguration().orientation == android.content.res.Configuration.ORIENTATION_PORTRAIT);
        backdrop.setFade(canvas, new float[] {0f, 0.55f, 1f}, new float[] {0f, 0.2f, 0.92f});
        backdrop.setDim(AppTheme.isHighContrast(activity) ? 0.85f : 1f);
        backdrop.setAmbientListener((bitmap, animate) -> host.setAmbient(bitmap, animate, 0));

        dialog.setCancelable(true);
        dialog.setCanceledOnTouchOutside(false);
        dialog.setOnCancelListener(ignored -> onCancel.run());
        cancel.setOnClickListener(v -> dialog.cancel());
        DpadEchoFilter echoes = new DpadEchoFilter();
        dialog.setOnKeyListener((target, key, event) -> {
            if (echoes.drop(event)) return true;
            if (!event.hasNoModifiers() || (key != KeyEvent.KEYCODE_BUTTON_B && key != KeyEvent.KEYCODE_ESCAPE)) return false;
            if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) dialog.onBackPressed();
            return true;
        });
        host.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            if (r - l != or - ol || b - t != ob - ot) v.post(this::fit);
        });
        showCover(null);
    }

    /** Opens the screen, on the main thread. {@code word} is the big word, {@code detail} the small line under it. Null when the activity is ending. */
    public static ConnectingScreen show(Activity activity, String title, String pc, String word, String detail, Runnable onCancel) {
        if (activity.isFinishing() || activity.isDestroyed()) return null;
        ConnectingScreen screen = new ConnectingScreen(activity, title, pc, onCancel);
        screen.word.setText(word);
        screen.setDetail(detail);
        screen.dialog.show();
        screen.orbit.setMoving(true);
        screen.focusCancelSoon();
        return screen;
    }

    /** The press that started the game can come twice: for a moment Cancel can't be pressed by keys. Back and B work throughout. */
    private void focusCancelSoon() {
        cancel.setFocusable(false);
        cancel.postDelayed(() -> {
            cancel.setFocusable(true);
            if (!dismissed) focusFirst(cancel);
        }, CANCEL_FOCUS_MS);
    }

    /** Reads a cover at no more than the size this window can show, off the main thread. Null when there is none or no memory for it. */
    public static Bitmap readCover(String path, int windowWidth, int windowHeight) {
        return readCover(path, windowWidth, windowHeight, android.graphics.BitmapFactory::decodeFile);
    }

    @androidx.annotation.VisibleForTesting
    interface CoverDecoder {
        Bitmap decode(String path, android.graphics.BitmapFactory.Options options);
    }

    @androidx.annotation.VisibleForTesting
    static Bitmap readCover(String path, int windowWidth, int windowHeight, CoverDecoder decoder) {
        android.graphics.BitmapFactory.Options options = new android.graphics.BitmapFactory.Options();
        try {
            options.inJustDecodeBounds = true;
            decoder.decode(path, options);
            options.inJustDecodeBounds = false;
            options.inSampleSize = sampleSize(options.outWidth, options.outHeight, windowWidth, windowHeight);
            return decoder.decode(path, options);
        } catch (OutOfMemoryError e) {
            com.limelight.LimeLog.warning("No memory for the connecting cover " + options.outWidth + "x" + options.outHeight + ": " + e);
            return null;
        }
    }

    /** The largest power of two that still leaves the picture covering the window. */
    @androidx.annotation.VisibleForTesting
    static int sampleSize(int width, int height, int windowWidth, int windowHeight) {
        int sample = 1;
        while (windowWidth > 0 && windowHeight > 0 && width / (sample * 2) >= windowWidth && height / (sample * 2) >= windowHeight) sample *= 2;
        return sample;
    }

    /** Opens straight on a failure, with the cover of what failed. */
    public static ConnectingScreen failure(Activity activity, String title, String pc, Bitmap cover, CharSequence heading, CharSequence message, Failure actions) {
        ConnectingScreen screen = new ConnectingScreen(activity, title, pc, actions::onBack);
        screen.showCover(cover);
        screen.fail(heading, message, actions);
        return screen;
    }

    public Dialog dialog() {
        return dialog;
    }

    public boolean isOrbitMoving() {
        return orbit.isTurning();
    }

    /** The stream's settings in a line, {@code 1920 × 1080, 60 FPS, 20 Mbps}. */
    public static String settingsLine(android.content.Context context, int width, int height, int fps, int bitrateKbps) {
        return width + " × " + height + ", " + fps + " FPS, " + BitrateSlider.text(context, bitrateKbps);
    }

    /** The word changes when the protocol's stage does. The small line says only what the first word needs. */
    public void setWord(String text) {
        activity.runOnUiThread(() -> word.setText(text));
    }

    public void setDetail(String text) {
        activity.runOnUiThread(() -> {
            line.setText(text);
            line.setVisibility(text == null || text.isEmpty() ? View.GONE : View.VISIBLE);
        });
    }

    /** The stage has moved on: the word changes and the small line has nothing left to add. */
    public void setStage(String text) {
        setWord(text);
        setDetail(null);
    }

    /** The cover, or null for the title's tile. The ambient comes from the same art. */
    public void showCover(Bitmap cover) {
        activity.runOnUiThread(() -> {
            if (dismissed) return;
            coverImage.setImageBitmap(cover);
            coverImage.setVisibility(cover == null ? View.GONE : View.VISIBLE);
            coverTile.setVisibility(cover == null ? View.VISIBLE : View.GONE);
            backdrop.focus("connecting:" + title, cover, cover == null, title, true);
        });
    }

    public void dismiss() {
        dismissed = true;
        activity.runOnUiThread(() -> {
            orbit.setMoving(false);
            if (dialog.isShowing()) dialog.dismiss();
        });
    }

    /** The cover moves up and the error's panel comes in below it. The orbit stops: nothing is under way now. */
    private void fail(CharSequence heading, CharSequence message, Failure actions) {
        failed = true;
        orbit.setMoving(false);
        orbit.setBare(true);
        stage.setVisibility(View.GONE);
        cancel.setVisibility(View.GONE);
        panel.setVisibility(View.VISIBLE);
        ((TextView) dialog.findViewById(R.id.conn_error_title)).setText(heading);
        TextView what = dialog.findViewById(R.id.conn_error_what);
        TextView pc = dialog.findViewById(R.id.conn_pc);
        what.setText(pc.getText().length() == 0 ? title : title + ", " + pc.getText());
        what.setVisibility(what.getText().length() == 0 ? View.GONE : View.VISIBLE);
        ((TextView) dialog.findViewById(R.id.conn_error_message)).setText(message);
        ActionSheet.styleRowGroup(dialog.findViewById(R.id.conn_rows));
        MenuRow settings = dialog.findViewById(R.id.conn_settings);
        CharSequence facts = actions.streamSettings();
        settings.setVisibility(facts == null ? View.GONE : View.VISIBLE);
        if (facts != null) {
            settings.setDetail(activity.getString(R.string.console_detail_profile), facts);
            settings.setOnClickListener(v -> {
                dismiss();
                actions.onStreamSettings();
            });
        }
        View retry = dialog.findViewById(R.id.conn_retry);
        retry.setOnClickListener(v -> {
            dismiss();
            actions.onTryAgain();
        });
        dialog.findViewById(R.id.conn_back).setOnClickListener(v -> dialog.cancel());
        TextView result = dialog.findViewById(R.id.conn_test_result);
        View test = dialog.findViewById(R.id.conn_test);
        View working = dialog.findViewById(R.id.conn_test_working);
        test.setOnClickListener(v -> {
            // Stays enabled, a disabled row would hand a remote's focus to the row above it.
            if (working.getVisibility() == View.VISIBLE) return;
            working.setVisibility(View.VISIBLE);
            result.setVisibility(View.GONE);
            new Thread(() -> {
                CharSequence words = actions.testNetwork();
                activity.runOnUiThread(() -> {
                    working.setVisibility(View.GONE);
                    result.setText(words);
                    result.setVisibility(View.VISIBLE);
                });
            }, "Network test").start();
        });
        focusFirst(retry);
    }

    public void showFailure() {
        dialog.show();
        fit();
    }

    /** A window starts in touch mode: one opened by keys asks the stream's window behind it which it was. */
    private void focusFirst(View view) {
        boolean byKeys = !activity.getWindow().getDecorView().isInTouchMode() || InputSource.last() == InputSource.PAD || InputSource.last() == InputSource.REMOTE;
        if (byKeys) view.post(view::requestFocusFromTouch);
    }

    /** Sizes by what the window has. A phone in portrait stacks; anything wider puts the words beside the cover. */
    private void fit() {
        View host = dialog.findViewById(R.id.conn_host);
        int width = host.getWidth();
        int height = host.getHeight();
        if (width == 0 || height == 0) return;
        boolean wide = width > height;
        float density = host.getResources().getDisplayMetrics().density;
        int orbitSize;
        int coverHeight;
        if (wide) {
            orbitSize = Math.round(height * (failed ? 0.72f : 0.82f));
            coverHeight = Math.round(orbitSize * (failed ? 0.96f : 0.68f));
        } else if (failed) {
            coverHeight = Math.round(Math.min(width * 0.55f, height * 0.27f));
            orbitSize = Math.round(coverHeight * 1.1f);
        } else {
            orbitSize = Math.round(Math.min(width * 0.92f, height * 0.5f));
            coverHeight = Math.round(orbitSize * 0.8f);
        }
        ViewGroup.LayoutParams o = orbit.getLayoutParams();
        o.width = orbitSize;
        o.height = orbitSize;
        orbit.setLayoutParams(o);
        View frame = dialog.findViewById(R.id.console_cover_frame);
        ViewGroup.LayoutParams f = frame.getLayoutParams();
        f.height = coverHeight;
        f.width = coverHeight * 2 / 3;
        frame.setLayoutParams(f);
        ViewGroup.LayoutParams c = coverCard.getLayoutParams();
        c.width = f.width;
        coverCard.setLayoutParams(c);
        ViewGroup.LayoutParams p = panel.getLayoutParams();
        p.width = wide ? Math.round(Math.min(density * 440, width * 0.55f)) : ViewGroup.LayoutParams.MATCH_PARENT;
        panel.setLayoutParams(p);
    }
}
