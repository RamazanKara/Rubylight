package com.limelight.ui;

import android.content.Context;
import android.os.Handler;
import android.os.Looper;
import android.widget.EditText;
import android.widget.SeekBar;

import java.math.BigDecimal;

/**
 * The bitrate slider in the stream menu's bitrate dialogs, ported from Moonlight V+
 * (qiin2333/moonlight-android, {@code gamemenu/BitrateCardController.kt}) like the desktop's:
 * V+'s piecewise scale from 500 kbps to 800 Mbps, fine steps where a few Mbps matter and coarse
 * ones above. Each step writes the dialog's bitrate field; the dialog's own OK path sends it, at
 * once when a drag ends and {@link #SETTLE_MS} after the last D-pad or key step.
 */
public final class BitrateSlider {
    static final int MIN_KBPS = 500;
    static final int MAX_KBPS = 800_000;
    // Each segment's top and step, in kbps.
    private static final int[][] SEGMENTS = {
            {5_000, 500}, {20_000, 1_000}, {50_000, 2_000}, {100_000, 5_000},
            {200_000, 10_000}, {400_000, 50_000}, {800_000, 100_000}};
    static final long SETTLE_MS = 450;
    public static final int MAX_PROGRESS = positions();

    private BitrateSlider() {
    }

    private static int positions() {
        int at = 0, from = MIN_KBPS;
        for (int[] segment : SEGMENTS) {
            at += (segment[0] - from) / segment[1];
            from = segment[0];
        }
        return at;
    }

    /** The bitrate at a slider position. */
    public static int kbps(int progress) {
        int at = 0, from = MIN_KBPS;
        progress = Math.max(0, Math.min(MAX_PROGRESS, progress));
        for (int[] segment : SEGMENTS) {
            int steps = (segment[0] - from) / segment[1];
            if (progress <= at + steps) return from + (progress - at) * segment[1];
            at += steps;
            from = segment[0];
        }
        return MAX_KBPS;
    }

    /** The nearest slider position to a bitrate. */
    public static int progress(int kbps) {
        kbps = Math.max(MIN_KBPS, Math.min(MAX_KBPS, kbps));
        int at = 0, from = MIN_KBPS;
        for (int[] segment : SEGMENTS) {
            if (kbps <= segment[0]) return at + Math.round((kbps - from) / (float) segment[1]);
            at += (segment[0] - from) / segment[1];
            from = segment[0];
        }
        return MAX_PROGRESS;
    }

    /** The bitrate as the dialogs' fields take it, in Mbps. */
    static String mbpsText(int kbps) {
        return BigDecimal.valueOf(kbps, 3).stripTrailingZeros().toPlainString();
    }

    /** A slider at {@code kbps} that fills {@code field} and runs {@code commit} when it settles. */
    public static SeekBar create(Context context, EditText field, int kbps, Runnable commit) {
        SeekBar bar = new SeekBar(context);
        bar.setMax(MAX_PROGRESS);
        bar.setProgress(progress(kbps));
        bar.setKeyProgressIncrement(1);
        bar.setOnSeekBarChangeListener(new Settle(field, commit, new Handler(Looper.getMainLooper())));
        return bar;
    }

    static final class Settle implements SeekBar.OnSeekBarChangeListener {
        private final EditText field;
        private final Handler handler;
        private final Runnable settled;
        private boolean tracking, pending;

        Settle(EditText field, Runnable commit, Handler handler) {
            this.field = field;
            this.handler = handler;
            this.settled = () -> {
                if (!pending) return;
                pending = false;
                commit.run();
            };
        }

        @Override
        public void onProgressChanged(SeekBar bar, int progress, boolean fromUser) {
            if (!fromUser) return;
            field.setText(mbpsText(kbps(progress)));
            pending = true;
            handler.removeCallbacks(settled);
            if (!tracking) handler.postDelayed(settled, SETTLE_MS);
        }

        @Override
        public void onStartTrackingTouch(SeekBar bar) {
            tracking = true;
            handler.removeCallbacks(settled);
        }

        @Override
        public void onStopTrackingTouch(SeekBar bar) {
            tracking = false;
            handler.removeCallbacks(settled);
            settled.run();
        }
    }
}
