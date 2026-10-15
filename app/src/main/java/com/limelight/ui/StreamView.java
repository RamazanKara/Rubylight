package com.limelight.ui;

import android.content.Context;
import android.os.SystemClock;
import android.text.InputType;
import android.util.AttributeSet;
import android.view.Gravity;
import android.view.KeyCharacterMap;
import android.view.KeyEvent;
import android.view.SurfaceView;
import android.view.SurfaceHolder;
import android.view.TextureView;
import android.view.ViewGroup;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.widget.FrameLayout;

public class StreamView extends FrameLayout {
    private final VideoScaling scaling = new VideoScaling();
    private int scalingMode = VideoScaling.STRETCH, alignment = VideoScaling.CENTER;
    private int videoWidth, videoHeight;
    private InputCallbacks inputCallbacks;
    private SurfaceView surfaceView;
    private TextureView textureView;

    public void initializeSurface(boolean useTextureView) {
        if (useTextureView) {
            textureView = new TextureView(getContext());
            addView(textureView, new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT));
        } else {
            surfaceView = new SurfaceView(getContext());
            addView(surfaceView, new LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT));
        }
    }

    public SurfaceHolder getHolder() {
        return surfaceView.getHolder();
    }

    public TextureView getTextureView() {
        return textureView;
    }

    public void setVideoTransform(float scale, float x, float y) {
        android.view.View video = textureView != null ? textureView : surfaceView;
        if (video == null) return;
        video.setPivotX(0); video.setPivotY(0);
        video.setScaleX(scale); video.setScaleY(scale);
        video.setTranslationX(x); video.setTranslationY(y);
    }

    /**
     * Sizes this view to the video's rectangle in the space its parent gives it, which may be larger
     * than that space (Fill, 1:1). Touch and pointer mapping work in this view's coordinates, so they
     * follow every mode without their own transform.
     */
    public void setVideoScaling(int mode, int alignment, int videoWidth, int videoHeight) {
        if (mode == scalingMode && alignment == this.alignment &&
                videoWidth == this.videoWidth && videoHeight == this.videoHeight) {
            return;
        }
        scalingMode = mode;
        this.alignment = alignment;
        this.videoWidth = videoWidth;
        this.videoHeight = videoHeight;
        applyGravity();
        requestLayout();
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        applyGravity();
    }

    private void applyGravity() {
        ViewGroup.LayoutParams params = getLayoutParams();
        if (!(params instanceof FrameLayout.LayoutParams)) return;
        FrameLayout.LayoutParams frame = (FrameLayout.LayoutParams) params;
        int gravity = VideoScaling.alignsTop(scalingMode, alignment) ?
                Gravity.TOP | Gravity.CENTER_HORIZONTAL : Gravity.CENTER;
        if (frame.gravity != gravity) {
            frame.gravity = gravity;
            setLayoutParams(frame);
        }
    }

    public void setInputCallbacks(InputCallbacks callbacks) {
        this.inputCallbacks = callbacks;
    }

    public StreamView(Context context) {
        super(context);
    }

    public StreamView(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    public StreamView(Context context, AttributeSet attrs, int defStyleAttr) {
        super(context, attrs, defStyleAttr);
    }

    public StreamView(Context context, AttributeSet attrs, int defStyleAttr, int defStyleRes) {
        super(context, attrs, defStyleAttr, defStyleRes);
    }

    @Override
    public boolean onCheckIsTextEditor() {
        return true;
    }

    @Override
    public InputConnection onCreateInputConnection(EditorInfo outAttrs) {
        outAttrs.inputType = InputType.TYPE_NULL;
        outAttrs.imeOptions = EditorInfo.IME_ACTION_NONE | EditorInfo.IME_FLAG_NO_EXTRACT_UI |
                EditorInfo.IME_FLAG_NO_FULLSCREEN;
        return new BaseInputConnection(this, false) {
            @Override
            public boolean commitText(CharSequence text, int newCursorPosition) {
                getEditable().clear();
                return inputCallbacks != null && inputCallbacks.handleTextInput(text);
            }

            @Override
            public boolean deleteSurroundingText(int beforeLength, int afterLength) {
                // Some IMEs use deletion calls even with TYPE_NULL. The text lives on the host.
                for (int i = 0; i < beforeLength; i++) {
                    sendKeyPress(KeyEvent.KEYCODE_DEL);
                }
                for (int i = 0; i < afterLength; i++) {
                    sendKeyPress(KeyEvent.KEYCODE_FORWARD_DEL);
                }
                return true;
            }

            @Override
            public boolean deleteSurroundingTextInCodePoints(int beforeLength, int afterLength) {
                return deleteSurroundingText(beforeLength, afterLength);
            }

            private void sendKeyPress(int keyCode) {
                long now = SystemClock.uptimeMillis();
                sendKeyEvent(new KeyEvent(now, now, KeyEvent.ACTION_DOWN, keyCode, 0, 0,
                        KeyCharacterMap.VIRTUAL_KEYBOARD, 0,
                        KeyEvent.FLAG_SOFT_KEYBOARD | KeyEvent.FLAG_KEEP_TOUCH_MODE));
                sendKeyEvent(new KeyEvent(now, now, KeyEvent.ACTION_UP, keyCode, 0, 0,
                        KeyCharacterMap.VIRTUAL_KEYBOARD, 0,
                        KeyEvent.FLAG_SOFT_KEYBOARD | KeyEvent.FLAG_KEEP_TOUCH_MODE));
            }
        };
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        if (scalingMode == VideoScaling.STRETCH || videoWidth <= 0 || videoHeight <= 0) {
            super.onMeasure(widthMeasureSpec, heightMeasureSpec);
            return;
        }
        scaling.layout(scalingMode, alignment, videoWidth, videoHeight,
                MeasureSpec.getSize(widthMeasureSpec), MeasureSpec.getSize(heightMeasureSpec));
        super.onMeasure(MeasureSpec.makeMeasureSpec(scaling.width(), MeasureSpec.EXACTLY),
                MeasureSpec.makeMeasureSpec(scaling.height(), MeasureSpec.EXACTLY));
    }

    @Override
    public boolean onKeyPreIme(int keyCode, KeyEvent event) {
        // This callbacks allows us to override dumb IME behavior like when
        // Samsung's default keyboard consumes Shift+Space.
        if (inputCallbacks != null) {
            if (event.getAction() == KeyEvent.ACTION_DOWN) {
                if (inputCallbacks.handleKeyDown(event)) {
                    return true;
                }
            }
            else if (event.getAction() == KeyEvent.ACTION_UP) {
                if (inputCallbacks.handleKeyUp(event)) {
                    return true;
                }
            }
        }

        return super.onKeyPreIme(keyCode, event);
    }

    public interface InputCallbacks {
        boolean handleKeyUp(KeyEvent event);
        boolean handleKeyDown(KeyEvent event);
        boolean handleTextInput(CharSequence text);
    }
}
