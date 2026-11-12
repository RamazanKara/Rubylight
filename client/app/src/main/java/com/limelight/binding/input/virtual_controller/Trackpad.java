package com.limelight.binding.input.virtual_controller;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.view.MotionEvent;

import com.limelight.nvstream.jni.MoonBridge;

/**
 * One of a Steam Controller's trackpads: a finger on it touches that pad on the PC, without a
 * click. One finger per pad, as on the controller.
 */
public class Trackpad extends VirtualControllerElement {
    private final int pad;
    private final String label;
    private final Paint paint = new Paint();
    private final RectF rect = new RectF();

    private boolean touching;
    private float lastX, lastY;

    public Trackpad(VirtualController controller, int elementId, int pad, String label, Context context) {
        super(controller, context, elementId);
        this.pad = pad;
        this.label = label;
    }

    @Override
    protected void onElementDraw(Canvas canvas) {
        float stroke = getDefaultStrokeWidth();
        paint.setStrokeWidth(stroke);
        paint.setColor(touching ? pressedColor : getDefaultColor());
        paint.setStyle(Paint.Style.STROKE);
        rect.set(stroke, stroke, getWidth() - stroke, getHeight() - stroke);
        float corner = Math.min(getWidth(), getHeight()) * 0.2f;
        canvas.drawRoundRect(rect, corner, corner, paint);

        paint.setStyle(Paint.Style.FILL_AND_STROKE);
        paint.setStrokeWidth(stroke / 2);
        paint.setTextAlign(Paint.Align.CENTER);
        paint.setTextSize(getPercent(getCorrectWidth(), 18));
        canvas.drawText(label, getWidth() / 2f, getHeight() / 2f + paint.getTextSize() / 3f, paint);

        if (touching) {
            paint.setStyle(Paint.Style.FILL);
            canvas.drawCircle(lastX, lastY, getCorrectWidth() * 0.08f, paint);
        }
    }

    @Override
    public boolean onElementTouchEvent(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                touching = true;
                send(MoonBridge.LI_TOUCH_EVENT_DOWN, event);
                break;
            case MotionEvent.ACTION_MOVE:
                if (touching) send(MoonBridge.LI_TOUCH_EVENT_MOVE, event);
                break;
            case MotionEvent.ACTION_UP:
                if (touching) send(MoonBridge.LI_TOUCH_EVENT_UP, event);
                touching = false;
                break;
            case MotionEvent.ACTION_CANCEL:
                // Also how the controls release everything; the last place stands in for the event's.
                if (touching) {
                    virtualController.sendControllerTouch(MoonBridge.LI_TOUCH_EVENT_CANCEL,
                            SteamControls.pointerId(pad), SteamControls.touchX(pad, fraction(lastX, getWidth())),
                            SteamControls.touchY(fraction(lastY, getHeight())), 0f);
                }
                touching = false;
                break;
            default:
                return true;
        }
        invalidate();
        return true;
    }

    private void send(byte type, MotionEvent event) {
        lastX = event.getX();
        lastY = event.getY();
        virtualController.sendControllerTouch(type, SteamControls.pointerId(pad),
                SteamControls.touchX(pad, fraction(lastX, getWidth())),
                SteamControls.touchY(fraction(lastY, getHeight())),
                SteamControls.pressure(event.getPressure()));
    }

    private static float fraction(float position, int size) {
        return size > 0 ? position / size : 0f;
    }
}
