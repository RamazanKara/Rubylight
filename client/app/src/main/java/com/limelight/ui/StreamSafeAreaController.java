package com.limelight.ui;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.Context;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.hardware.display.DisplayManager;
import android.os.Handler;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.Display;
import android.widget.LinearLayout;
import android.widget.SeekBar;
import android.widget.TextView;
import android.widget.Toast;

import java.util.function.IntSupplier;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;

/**
 * The stream's phone-only framing: the picture position the player chose for this screen
 * ({@link PicturePosition}) applied to the video, and Flip 180°, which turns a landscape stream
 * over until it ends. Android does not restart or reconfigure the activity on a 180° turn, so a
 * display listener re-applies the offset, which stays on the same physical side of the phone.
 * Nothing here talks to the host: a flip keeps the stream's size.
 */
public final class StreamSafeAreaController implements DisplayManager.DisplayListener {
    private final Activity activity;
    private StreamView view;
    private int lastRotation = -1;
    /** Flip 180°'s orientation for the rest of the stream, or -1 to follow the Orientation setting. */
    private int flipped = -1;

    public StreamSafeAreaController(Activity activity) {
        this.activity = activity;
    }

    public void attach(StreamView streamView) {
        view = streamView;
        DisplayManager displays = (DisplayManager) activity.getSystemService(Context.DISPLAY_SERVICE);
        if (displays != null) displays.registerDisplayListener(this, new Handler(Looper.getMainLooper()));
        apply();
    }

    public void detach() {
        DisplayManager displays = (DisplayManager) activity.getSystemService(Context.DISPLAY_SERVICE);
        if (displays != null) displays.unregisterDisplayListener(this);
        view = null;
    }

    /** Moves the video by this screen's picture position; windows and TVs keep it centred. */
    public void apply() {
        if (view == null) return;
        Display display = activity.getWindowManager().getDefaultDisplay();
        lastRotation = display.getRotation();
        if (!applies()) {
            view.setPictureOffset(0, 0);
            return;
        }
        int[] offset = PicturePosition.screenOffset(prefs(), display);
        view.setPictureOffset(offset[0], offset[1]);
    }

    /** The orientation to request: Flip 180°'s while it holds, else the setting's. A window drops the flip. */
    public int requested(int fromSetting, boolean freeWindow) {
        if (freeWindow) flipped = -1;
        apply();
        return flipped >= 0 ? flipped : fromSetting;
    }

    /** Picture position applies to a full-screen stream on a phone's or tablet's own screen. */
    public boolean applies() {
        return !television(activity) && !activity.isInMultiWindowMode() && !activity.isInPictureInPictureMode();
    }

    /** Flip 180° is offered while a phone or tablet shows the stream in landscape. */
    public boolean canFlip() {
        return !television(activity) && flipTarget() >= 0;
    }

    /** Turns a landscape stream over; says why not in a window or on another display. */
    public void flip(boolean otherDisplay) {
        if (otherDisplay || activity.isInMultiWindowMode()) {
            Toast.makeText(activity, R.string.stream_flip_unavailable, Toast.LENGTH_LONG).show();
            return;
        }
        int target = flipTarget();
        if (target < 0) return;
        flipped = target;
        activity.setRequestedOrientation(target);
    }

    /** The Picture position dialog: two sliders that move the picture live, and Center. */
    public AlertDialog createPositionDialog() {
        Display display = activity.getWindowManager().getDefaultDisplay();
        return createPositionDialog(activity, display::getRotation, this::apply);
    }

    /**
     * Also used by Settings, where there is no picture to move: there the rotation is the one the
     * stream will show in ({@link PicturePosition#streamRotation}), so the sliders mean left, right,
     * up and down in the stream however Settings is held. In a stream it is the rotation now.
     */
    public static AlertDialog createPositionDialog(Context context, IntSupplier rotation, Runnable changed) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        Display display = ((android.view.WindowManager) context.getSystemService(Context.WINDOW_SERVICE)).getDefaultDisplay();
        int[] offset = PicturePosition.screenOffset(prefs, display, rotation.getAsInt());
        int max = SafeAreaPolicy.MAX_OFFSET_PERMILLE;
        LinearLayout content = new LinearLayout(context);
        content.setOrientation(LinearLayout.VERTICAL);
        int pad = Math.round(24 * context.getResources().getDisplayMetrics().density);
        content.setPadding(pad, pad / 2, pad, 0);
        SeekBar horizontal = slider(context, content, R.string.picture_position_horizontal, offset[0] + max, 2 * max);
        SeekBar vertical = slider(context, content, R.string.picture_position_vertical, offset[1] + max, 2 * max);
        SeekBar.OnSeekBarChangeListener listener = new SeekBar.OnSeekBarChangeListener() {
            @Override public void onProgressChanged(SeekBar bar, int progress, boolean fromUser) {
                if (!fromUser) return;
                PicturePosition.saveScreenOffset(prefs, display, rotation.getAsInt(), horizontal.getProgress() - max, vertical.getProgress() - max);
                changed.run();
            }
            @Override public void onStartTrackingTouch(SeekBar bar) {}
            @Override public void onStopTrackingTouch(SeekBar bar) {}
        };
        horizontal.setOnSeekBarChangeListener(listener);
        vertical.setOnSeekBarChangeListener(listener);
        return new NavigationDialogBuilder(context).setTitle(R.string.picture_position_title)
                .setMessage(R.string.summary_picture_position).setView(content)
                .setNeutralButton(R.string.picture_position_center, (dialog, which) -> {
                    PicturePosition.saveScreenOffset(prefs, display, rotation.getAsInt(), 0, 0);
                    changed.run();
                })
                .setPositiveButton(android.R.string.ok, null).create();
    }

    private static SeekBar slider(Context context, LinearLayout parent, int label, int progress, int max) {
        TextView title = new TextView(context);
        title.setText(label);
        parent.addView(title);
        SeekBar bar = new SeekBar(context);
        bar.setMax(max);
        bar.setProgress(progress);
        bar.setContentDescription(context.getString(label));
        parent.addView(bar);
        return bar;
    }

    private int flipTarget() {
        Display display = activity.getWindowManager().getDefaultDisplay();
        return StreamOrientation.flipped(display.getRotation(), PicturePosition.naturalPortrait(display));
    }

    private SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(activity);
    }

    private static boolean television(Context context) {
        return (context.getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION;
    }

    @Override public void onDisplayAdded(int displayId) {}

    @Override public void onDisplayRemoved(int displayId) {}

    // lastRotation starts at -1 (no rotation seen yet), which is not a Surface.ROTATION_* value.
    @SuppressLint("WrongConstant")
    @Override
    public void onDisplayChanged(int displayId) {
        // A 180° turn reaches only here: the cutout moved to the other side, and the offset with it.
        if (displayId != Display.DEFAULT_DISPLAY || view == null) return;
        if (activity.getWindowManager().getDefaultDisplay().getRotation() != lastRotation) apply();
    }
}
