package com.limelight.ui;

import android.content.Context;
import android.view.View;
import java.util.function.Consumer;

/** The stream's bounded desktop controls, retaining the original keyboard entry point. */
public final class KeyboardOverlay extends DesktopControlsPanel {
    public KeyboardOverlay(Context context, Sink sink, Runnable keyboard, Runnable hide) {
        super(context, sink, keyboard, hide);
    }

    /** Keyboard-only adapters remain useful to previews which do not have a pointer surface. */
    public KeyboardOverlay(Context context, Consumer<int[]> sendKeys, Runnable keyboard, Runnable hide) {
        this(context, new Sink() {
            @Override public void sendKeys(int[] keys) { sendKeys.accept(keys); }
            @Override public void move(float dx, float dy) { }
            @Override public void scroll(float vertical, float horizontal) { }
            @Override public void mouseButton(int button, boolean down) { }
        }, keyboard, hide);
    }

    @Override protected void onMeasure(int widthSpec, int heightSpec) {
        // Keep part of the game visible, including short landscape windows. The panel retains
        // its header and scrolls its content when the chosen key size cannot fit.
        int available = View.MeasureSpec.getSize(heightSpec);
        if (available > 0 && View.MeasureSpec.getMode(heightSpec) != View.MeasureSpec.UNSPECIFIED) {
            heightSpec = View.MeasureSpec.makeMeasureSpec(Math.round(available * .78f), View.MeasureSpec.AT_MOST);
        }
        super.onMeasure(widthSpec, heightSpec);
    }
}
