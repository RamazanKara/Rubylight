package com.limelight.binding.input.virtual_controller;

import android.content.Context;
import android.view.MotionEvent;

/**
 * A button that holds its flag from one tap to the next, for a Steam Controller's grip touch:
 * a hand rests on a grip for a whole game, which a finger on the screen cannot. Its own layer, so
 * a finger sliding off another button never presses it.
 */
public class LatchingButton extends DigitalButton {
    private static final int LAYER = -1;

    private final int flag;
    private boolean latched;

    public LatchingButton(VirtualController controller, int elementId, int flag, Context context) {
        super(controller, elementId, LAYER, context);
        this.flag = flag;
    }

    public boolean isLatched() {
        return latched;
    }

    @Override
    public boolean onElementTouchEvent(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                setLatched(!latched);
                break;
            case MotionEvent.ACTION_CANCEL:
                // The controls release everything, so the grip lets go too.
                setLatched(false);
                break;
            default:
                break;
        }
        return true;
    }

    private void setLatched(boolean latched) {
        if (this.latched == latched) return;
        this.latched = latched;
        VirtualController.ControllerInputContext inputContext = virtualController.getControllerInputContext();
        if (latched) {
            inputContext.inputMap |= flag;
        } else {
            inputContext.inputMap &= ~flag;
        }
        setPressed(latched);
        invalidate();
        virtualController.sendControllerInputContext();
    }
}
