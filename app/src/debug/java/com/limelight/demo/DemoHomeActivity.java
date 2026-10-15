package com.limelight.demo;

import android.app.Activity;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Typeface;
import android.os.Bundle;
import android.view.View;

import androidx.core.content.ContextCompat;

import com.limelight.R;

/**
 * A plain stand-in home screen for the picture-in-picture capture. It is disabled in the manifest and only
 * enabled, and made the preferred home, by scripts/demo/capture-common.ps1 for the duration of a capture, so
 * the floating stream sits on a clean dark background instead of a launcher full of third-party icons.
 * It uses the Nightjar palette: flat night, amber moon rings and ink text.
 */
public final class DemoHomeActivity extends Activity {
    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        setContentView(new Stage(this));
        getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_STABLE |
                View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);
    }

    private static final class Stage extends View {
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);

        Stage(Context context) {
            super(context);
        }

        @Override protected void onDraw(Canvas canvas) {
            int w = getWidth(), h = getHeight();
            float s = w / 1080f;
            canvas.drawColor(ContextCompat.getColor(getContext(), R.color.brand_night));

            // Concentric rings echo the README banner. They are centred where Android parks the picture-in-picture
            // window (bottom right, above the gesture bar), so the floating stream is the focal point.
            paint.setStyle(Paint.Style.STROKE);
            paint.setStrokeWidth(2 * s);
            paint.setColor(0x36f2a93b);
            for (int ring = 1; ring <= 6; ring++) canvas.drawCircle(w * 0.685f, h * 0.885f, ring * 170 * s, paint);

            paint.setStyle(Paint.Style.FILL);
            paint.setColor(0xfff4efe6);
            paint.setTypeface(Typeface.create("sans-serif-light", Typeface.NORMAL));
            // Clock and date at the top left, like a lock screen, well clear of the window in the bottom-right corner.
            paint.setTextSize(150 * s);
            canvas.drawText("10:30", 80 * s, 520 * s, paint);
            paint.setTypeface(Typeface.create("sans-serif", Typeface.NORMAL));
            paint.setTextSize(46 * s);
            paint.setColor(0xffc1c4d2);
            canvas.drawText("Friday, October 9", 88 * s, 616 * s, paint);
        }
    }
}
