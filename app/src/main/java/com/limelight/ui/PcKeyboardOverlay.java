package com.limelight.ui;

import android.content.Context;
import android.graphics.Color;
import android.graphics.Typeface;
import android.view.Gravity;
import android.view.View;
import android.widget.ImageButton;
import android.widget.LinearLayout;
import android.widget.TextView;

import com.limelight.R;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.binding.input.keyboard.PcKeyboardEngine;
import com.limelight.binding.input.keyboard.PcKeyboardLayout;

import java.util.Locale;

/**
 * The full PC keyboard over the bottom of the stream: a thin header with the layout name and a close
 * button, then the keys. Touch, controller and remote all drive the same {@link PcKeyboardController}.
 */
public final class PcKeyboardOverlay extends LinearLayout {
    private final PcKeyboardController controller;
    private final PcKeyboardView keys;
    private final TextView layoutName;
    private final int sizePercent;

    public PcKeyboardOverlay(Context context, String layoutPreference, int sizePercent, int opacityPercent,
                             PcKeyboardEngine.Output output, Runnable close) {
        super(context);
        this.sizePercent = sizePercent;
        float density = context.getResources().getDisplayMetrics().density;
        setOrientation(VERTICAL);
        setBackgroundColor(0xE6101216);
        setAlpha(opacityPercent / 100f);
        // Keys must not take focus from the stream or draw the system focus ring around the panel.
        setFocusable(false);
        setClickable(true);

        PcKeyboardView[] view = new PcKeyboardView[1];
        controller = new PcKeyboardController(PcKeyboardLayout.resolve(layoutPreference, Locale.getDefault()), output,
                new PcKeyboardController.Host() {
                    @Override public void close() { close.run(); }
                    @Override public void changed() { if (view[0] != null) view[0].changed(); }
                });
        keys = new PcKeyboardView(context, controller);
        view[0] = keys;

        LinearLayout header = new LinearLayout(context);
        header.setOrientation(HORIZONTAL);
        header.setGravity(Gravity.CENTER_VERTICAL);
        int pad = Math.round(8 * density);
        header.setPadding(pad, 0, 0, 0);

        TextView title = new TextView(context);
        title.setText(R.string.pc_keyboard_title);
        title.setTextColor(0xFFF2F4F7);
        title.setTypeface(Typeface.DEFAULT_BOLD);
        title.setTextSize(13);
        header.addView(title, new LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT));

        // Touch users switch layouts here for this stream; the setting picks the starting one.
        layoutName = new TextView(context);
        layoutName.setTextColor(0xFF9AA3AF);
        layoutName.setTextSize(13);
        layoutName.setPadding(pad * 2, pad, pad * 2, pad);
        layoutName.setOnClickListener(v -> cycleLayout());
        layoutName.setContentDescription(context.getString(R.string.pc_keyboard_layout_title));
        header.addView(layoutName, new LayoutParams(LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT));
        showLayoutName();

        View spacer = new View(context);
        header.addView(spacer, new LayoutParams(0, 1, 1f));

        ImageButton closeButton = new ImageButton(context);
        closeButton.setImageResource(R.drawable.ic_close);
        closeButton.setColorFilter(Color.WHITE);
        closeButton.setBackground(null);
        closeButton.setContentDescription(context.getString(R.string.pc_keyboard_close));
        closeButton.setFocusable(false);
        closeButton.setOnClickListener(v -> close.run());
        int button = Math.round(36 * density);
        header.addView(closeButton, new LayoutParams(button, button));

        addView(header, new LayoutParams(LayoutParams.MATCH_PARENT, Math.round(34 * density)));
        addView(keys, new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.WRAP_CONTENT));
    }

    public PcKeyboardController controller() {
        return controller;
    }

    /** Everything this panel pressed on the host is let go; call when it closes or the stream loses input. */
    public void releaseInputs() {
        controller.release();
        keys.changed();
    }

    private void cycleLayout() {
        String[] ids = PcKeyboardLayout.IDS;
        int next = 0;
        for (int i = 0; i < ids.length; i++) {
            if (ids[i].equals(controller.layout().id)) next = (i + 1) % ids.length;
        }
        controller.setLayout(PcKeyboardLayout.byId(ids[next]));
        showLayoutName();
        keys.requestLayout();
        keys.changed();
    }

    private void showLayoutName() {
        layoutName.setText(PcKeyboardLayout.US.equals(controller.layout().id)
                ? R.string.pc_keyboard_layout_us : R.string.pc_keyboard_layout_de);
    }

    @Override
    protected void onMeasure(int widthSpec, int heightSpec) {
        int available = MeasureSpec.getSize(heightSpec);
        if (available > 0 && MeasureSpec.getMode(heightSpec) != MeasureSpec.UNSPECIFIED) {
            float density = getResources().getDisplayMetrics().density;
            int header = Math.round(34 * density);
            keys.setHeightLimit(Math.max(1, Math.round(available * sizePercent / 100f) - header
                    - getPaddingTop() - getPaddingBottom()));
        }
        super.onMeasure(widthSpec, heightSpec);
    }
}
