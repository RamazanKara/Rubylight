/**
 * Created by Karim Mreisi.
 */

package com.limelight.binding.input.virtual_controller;

import android.content.Context;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.util.DisplayMetrics;
import android.view.Gravity;
import android.view.LayoutInflater;
import android.view.MotionEvent;
import android.view.View;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.TextView;
import android.widget.Toast;

import com.google.android.material.slider.Slider;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.input.ControllerHandler;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

public class VirtualController {
    public static class ControllerInputContext {
        public int inputMap = 0;
        public byte leftTrigger = 0x00;
        public byte rightTrigger = 0x00;
        public short rightStickX = 0x0000;
        public short rightStickY = 0x0000;
        public short leftStickX = 0x0000;
        public short leftStickY = 0x0000;
    }

    /** Where the on-screen trackpads' touches go: the controller handler, or a test. */
    interface TouchSink {
        void sendTouch(byte eventType, int pointerId, float x, float y, float pressure);
    }

    public enum ControllerMode {
        Active,
        MoveButtons,
        ResizeButtons
    }

    private static final boolean _PRINT_DEBUG_INFORMATION = false;

    private final ControllerHandler controllerHandler;
    private final Context context;
    private final Handler handler;
    private TouchSink touchSink;

    private final Runnable delayedRetransmitRunnable = new Runnable() {
        @Override
        public void run() {
            sendControllerInputContextInternal();
        }
    };

    private FrameLayout frame_layout = null;

    ControllerMode currentMode = ControllerMode.Active;
    ControllerInputContext inputContext = new ControllerInputContext();

    private Button buttonConfigure = null;

    private List<VirtualControllerElement> elements = new ArrayList<>();

    // While resizing: the panel with the opacity slider, and the control it sets (null: every control).
    private View opacityPanel;
    private Slider opacitySlider;
    private TextView opacityValue;
    private VirtualControllerElement editing;
    private boolean settingOpacity;

    public VirtualController(final ControllerHandler controllerHandler, FrameLayout layout, final Context context) {
        this.controllerHandler = controllerHandler;
        this.frame_layout = layout;
        this.context = context;
        this.handler = new Handler(Looper.getMainLooper());
        if (controllerHandler != null) this.touchSink = controllerHandler::reportOscTouch;

        buttonConfigure = new Button(context);
        buttonConfigure.setAlpha(0.25f);
        buttonConfigure.setFocusable(false);
        buttonConfigure.setBackgroundResource(R.drawable.ic_settings);
        buttonConfigure.setContentDescription(context.getString(R.string.stream_controls_layout));
        buttonConfigure.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                if (currentMode == ControllerMode.Active){
                    setControllerMode(ControllerMode.MoveButtons);
                } else if (currentMode == ControllerMode.MoveButtons) {
                    setControllerMode(ControllerMode.ResizeButtons);
                } else {
                    setControllerMode(ControllerMode.Active);
                }
            }
        });

    }

    Handler getHandler() {
        return handler;
    }

    public void hide() {
        releaseInput();
        for (VirtualControllerElement element : elements) {
            element.setVisibility(View.INVISIBLE);
        }

        buttonConfigure.setVisibility(View.INVISIBLE);
        if (opacityPanel != null) opacityPanel.setVisibility(View.INVISIBLE);
    }

    public void show() {
        for (VirtualControllerElement element : elements) {
            element.setVisibility(View.VISIBLE);
        }

        buttonConfigure.setVisibility(View.VISIBLE);
        if (opacityPanel != null) opacityPanel.setVisibility(View.VISIBLE);
    }

    public void removeElements() {
        for (VirtualControllerElement element : elements) {
            frame_layout.removeView(element);
        }
        elements.clear();
        editing = null;

        frame_layout.removeView(buttonConfigure);
        if (opacityPanel != null) frame_layout.removeView(opacityPanel);
    }

    public void setOpacity(int opacity) {
        for (VirtualControllerElement element : elements) {
            element.setOpacity(opacity);
        }
    }


    public void addElement(VirtualControllerElement element, int x, int y, int width, int height) {
        elements.add(element);
        FrameLayout.LayoutParams layoutParams = new FrameLayout.LayoutParams(width, height);
        layoutParams.setMargins(x, y, 0, 0);

        frame_layout.addView(element, layoutParams);
    }

    public List<VirtualControllerElement> getElements() {
        return elements;
    }

    /** The visible controls and their layout button, in the coordinates of the layout they are in. */
    public void collectVisibleBounds(List<android.graphics.Rect> out) {
        for (VirtualControllerElement element : elements) {
            com.limelight.ui.OverlayPlacement.addVisible(element, frame_layout, out);
        }
        com.limelight.ui.OverlayPlacement.addVisible(buttonConfigure, frame_layout, out);
        if (opacityPanel != null && opacityPanel.getParent() != null) {
            com.limelight.ui.OverlayPlacement.addVisible(opacityPanel, frame_layout, out);
        }
    }

    private static final void _DBG(String text) {
        if (_PRINT_DEBUG_INFORMATION) {
            LimeLog.info("VirtualController: " + text);
        }
    }

    public void refreshLayout() {
        releaseInput();
        removeElements();

        DisplayMetrics screen = context.getResources().getDisplayMetrics();

        int buttonSize = Math.max(Math.round(48 * screen.density), (int)(screen.heightPixels*0.06f));
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(buttonSize, buttonSize);
        params.leftMargin = 15;
        params.topMargin = 15;
        frame_layout.addView(buttonConfigure, params);

        // Start with the default layout
        VirtualControllerConfigurationLoader.createDefaultLayout(this, context);

        // Apply user preferences onto the default layout
        VirtualControllerConfigurationLoader.loadFromPreferences(this, context);

        if (currentMode == ControllerMode.ResizeButtons) {
            showOpacityPanel();
        }
    }

    public ControllerMode getControllerMode() {
        return currentMode;
    }

    public void setControllerMode(ControllerMode mode) {
        if (mode == currentMode) {
            return;
        }
        releaseInput();
        currentMode = mode;
        if (mode == ControllerMode.Active) {
            VirtualControllerConfigurationLoader.saveProfile(this, context);
        }
        editing = null;
        if (mode == ControllerMode.ResizeButtons) {
            showOpacityPanel();
        } else if (opacityPanel != null) {
            frame_layout.removeView(opacityPanel);
        }
        Toast.makeText(context, mode == ControllerMode.MoveButtons ? R.string.stream_controls_move_help :
                mode == ControllerMode.ResizeButtons ? R.string.stream_controls_resize_help :
                        R.string.stream_controls_saved, Toast.LENGTH_SHORT).show();
        buttonConfigure.invalidate();
        for (VirtualControllerElement element : elements) {
            element.invalidate();
        }
    }

    /** Picks the control the opacity slider sets while resizing. */
    void selectForEditing(VirtualControllerElement element) {
        VirtualControllerElement previous = editing;
        editing = element;
        if (previous != null && previous != element) previous.invalidate();
        element.invalidate();
        updateOpacityPanel();
    }

    boolean isSelectedForEditing(VirtualControllerElement element) {
        return currentMode == ControllerMode.ResizeButtons && editing == element;
    }

    /** Sets the picked control's own opacity, in percent; every control's until one is picked. */
    void setEditingOpacity(int opacity) {
        if (editing != null) {
            editing.setButtonOpacity(opacity);
        } else {
            for (VirtualControllerElement element : elements) {
                element.setButtonOpacity(opacity);
            }
        }
        updateOpacityPanel();
    }

    /** The opacity the slider shows: the picked control's, or before one is picked the highest of all. */
    private int shownOpacity() {
        if (editing != null) return editing.getButtonOpacity();
        int highest = elements.isEmpty() ? VirtualControllerElement.MAX_BUTTON_OPACITY : VirtualControllerElement.MIN_BUTTON_OPACITY;
        for (VirtualControllerElement element : elements) {
            highest = Math.max(highest, element.getButtonOpacity());
        }
        return highest;
    }

    View getOpacityPanel() {
        return opacityPanel;
    }

    private void showOpacityPanel() {
        if (opacityPanel == null) {
            // The stream's dark surfaces, like the modern controller's editor panel.
            Context themed = new androidx.appcompat.view.ContextThemeWrapper(context, R.style.StreamTheme);
            opacityPanel = LayoutInflater.from(themed).inflate(R.layout.osc_opacity_panel, frame_layout, false);
            opacitySlider = opacityPanel.findViewById(R.id.osc_opacity_slider);
            opacityValue = opacityPanel.findViewById(R.id.osc_opacity_value);
            com.limelight.ui.StepSlider.showValue(opacitySlider, opacity -> String.format(Locale.getDefault(), "%d %%", opacity));
            opacitySlider.addOnChangeListener((slider, value, fromUser) -> {
                if (fromUser && !settingOpacity) setEditingOpacity(Math.round(value));
            });
        }
        if (opacityPanel.getParent() == null) {
            FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(
                    opacityPanel.getLayoutParams().width, FrameLayout.LayoutParams.WRAP_CONTENT,
                    Gravity.TOP | Gravity.CENTER_HORIZONTAL);
            params.topMargin = 15;
            frame_layout.addView(opacityPanel, params);
        }
        updateOpacityPanel();
    }

    private void updateOpacityPanel() {
        if (opacityPanel == null) return;
        int opacity = shownOpacity();
        settingOpacity = true;
        // The slider moves in 5 % steps; a stored value between them shows at the nearest one.
        opacitySlider.setValue(Math.max(VirtualControllerElement.MIN_BUTTON_OPACITY,
                Math.min(VirtualControllerElement.MAX_BUTTON_OPACITY, Math.round(opacity / 5f) * 5)));
        settingOpacity = false;
        // Usable at once: before a control is touched it sets them all (nightjar#107).
        opacitySlider.setEnabled(!elements.isEmpty());
        opacityValue.setText(String.format(Locale.getDefault(), "%d %%", opacity));
    }

    public void releaseInput() {
        if (elements.isEmpty()) {
            return;
        }
        long now = SystemClock.uptimeMillis();
        MotionEvent cancel = MotionEvent.obtain(now, now, MotionEvent.ACTION_CANCEL, 0, 0, 0);
        for (VirtualControllerElement element : elements) {
            element.onTouchEvent(cancel);
        }
        cancel.recycle();
        handler.removeCallbacksAndMessages(null);
        inputContext = new ControllerInputContext();
        sendControllerInputContextInternal();
    }

    public ControllerInputContext getControllerInputContext() {
        return inputContext;
    }

    private void sendControllerInputContextInternal() {
        _DBG("INPUT_MAP + " + inputContext.inputMap);
        _DBG("LEFT_TRIGGER " + inputContext.leftTrigger);
        _DBG("RIGHT_TRIGGER " + inputContext.rightTrigger);
        _DBG("LEFT STICK X: " + inputContext.leftStickX + " Y: " + inputContext.leftStickY);
        _DBG("RIGHT STICK X: " + inputContext.rightStickX + " Y: " + inputContext.rightStickY);

        if (controllerHandler != null) {
            controllerHandler.reportOscState(
                    inputContext.inputMap,
                    inputContext.leftStickX,
                    inputContext.leftStickY,
                    inputContext.rightStickX,
                    inputContext.rightStickY,
                    inputContext.leftTrigger,
                    inputContext.rightTrigger
            );
        }
    }

    void setTouchSink(TouchSink touchSink) {
        this.touchSink = touchSink;
    }

    /** A touch on one of the on-screen trackpads, for touchpad 0 of the controls' pad. */
    void sendControllerTouch(byte eventType, int pointerId, float x, float y, float pressure) {
        if (touchSink != null) touchSink.sendTouch(eventType, pointerId, x, y, pressure);
    }

    void sendControllerInputContext() {
        // Cancel retransmissions of prior gamepad inputs
        handler.removeCallbacks(delayedRetransmitRunnable);

        sendControllerInputContextInternal();

        // HACK: GFE sometimes discards gamepad packets when they are received
        // very shortly after another. This can be critical if an axis zeroing packet
        // is lost and causes an analog stick to get stuck. To avoid this, we retransmit
        // the gamepad state a few times unless another input event happens before then.
        handler.postDelayed(delayedRetransmitRunnable, 25);
        handler.postDelayed(delayedRetransmitRunnable, 50);
        handler.postDelayed(delayedRetransmitRunnable, 75);
    }
}
