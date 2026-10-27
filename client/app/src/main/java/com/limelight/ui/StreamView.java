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
import android.view.inputmethod.InputMethodManager;
import android.widget.FrameLayout;

public class StreamView extends FrameLayout {
    private final VideoScaling scaling = new VideoScaling();
    private int scalingMode = VideoScaling.STRETCH, alignment = VideoScaling.CENTER;
    private int videoWidth, videoHeight;
    // Decoded picture over the stream it carries, 1 unless the decoder leaves encoder padding on
    // the right and bottom uncropped (AV1 from AMD encoders at widths not divisible by 64).
    private float paddedScaleX = 1, paddedScaleY = 1;
    private InputCallbacks inputCallbacks;
    private HostTextField hostTextField;
    private boolean pressPosted;
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

    /**
     * Enlarges the video inside this view by the decoder's uncropped padding and clips it, so the
     * stream fills this view and lines up with touch and pointer mapping, which stay in this view's
     * coordinates.
     */
    public void setDecodedPadding(int decodedWidth, int decodedHeight, int shownWidth, int shownHeight) {
        float scaleX = 1, scaleY = 1;
        if (shownWidth > 0 && shownHeight > 0 && decodedWidth >= shownWidth && decodedHeight >= shownHeight) {
            scaleX = (float) decodedWidth / shownWidth;
            scaleY = (float) decodedHeight / shownHeight;
        }
        if (scaleX != paddedScaleX || scaleY != paddedScaleY) {
            paddedScaleX = scaleX;
            paddedScaleY = scaleY;
            requestLayout();
        }
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

    /**
     * The host's focused text field the soft keyboard types into, or null to send raw keys
     * (TYPE_NULL), as without text field notices. Restarts the IME's input with the field's type.
     */
    public void setHostTextField(HostTextField field) {
        if (field == hostTextField) return;
        // What the old field composed is already on the host: the IME starts over with the new one.
        if (hostTextField != null) hostTextField.finishComposing();
        hostTextField = field;
        InputMethodManager inputManager = (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
        if (inputManager != null) inputManager.restartInput(this);
    }

    /**
     * A tap or click on the stream may move the host's caret or focus before the host's notice
     * comes: the IME starts over on a fresh copy of the field instead of finishing a word against
     * the text it typed before. Posted, so the press itself reaches the host first.
     */
    public void onStreamPress(android.view.MotionEvent event) {
        int action = event.getActionMasked();
        if (hostTextField == null || pressPosted || (action != android.view.MotionEvent.ACTION_DOWN &&
                action != android.view.MotionEvent.ACTION_BUTTON_PRESS)) {
            return;
        }
        // A mouse click is both a touch down and a button press: one restart.
        pressPosted = true;
        post(() -> {
            pressPosted = false;
            HostTextField field = hostTextField;
            if (field != null) setHostTextField(field.fresh());
        });
    }

    public HostTextField getHostTextField() {
        return hostTextField;
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
        HostTextField field = hostTextField;
        if (field != null) {
            outAttrs.inputType = field.inputType();
            outAttrs.imeOptions = field.imeOptions();
            outAttrs.initialSelStart = outAttrs.initialSelEnd = field.cursor();
            return new HostFieldConnection(field);
        }
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

    /**
     * Forwards the IME's edits of the host's text field as they happen: composing text, commits and
     * deletions all go through {@link HostTextField} to {@link InputCallbacks#handleTextInput}, in
     * order, and the IME is told where its cursor and composing text now are.
     */
    private final class HostFieldConnection extends BaseInputConnection {
        private final HostTextField field;
        private final HostTextField.Sink sink = edit -> inputCallbacks != null && inputCallbacks.handleTextInput(edit);

        HostFieldConnection(HostTextField field) {
            super(StreamView.this, false);
            this.field = field;
        }

        @Override
        public boolean setComposingText(CharSequence text, int newCursorPosition) {
            return updated(field.compose(text, sink));
        }

        @Override
        public boolean commitText(CharSequence text, int newCursorPosition) {
            return updated(field.commit(text, sink));
        }

        @Override
        public boolean finishComposingText() {
            field.finishComposing();
            return updated(true);
        }

        @Override
        public boolean setComposingRegion(int start, int end) {
            return updated(field.recompose(start, end));
        }

        @Override
        public boolean deleteSurroundingText(int beforeLength, int afterLength) {
            boolean sent = field.deleteBefore(beforeLength, sink);
            forwardDelete(afterLength);
            return updated(sent);
        }

        @Override
        public boolean deleteSurroundingTextInCodePoints(int beforeLength, int afterLength) {
            boolean sent = field.deleteBeforeInCodePoints(beforeLength, sink);
            forwardDelete(afterLength);
            return updated(sent);
        }

        // Text after the caret has no character here: a forward delete per character goes straight
        // on, in order (per UTF-16 unit for deleteSurroundingText, which over-deletes an emoji).
        private void forwardDelete(int afterLength) {
            for (int i = 0; i < afterLength && inputCallbacks != null; i++) {
                long now = SystemClock.uptimeMillis();
                int flags = KeyEvent.FLAG_SOFT_KEYBOARD | KeyEvent.FLAG_KEEP_TOUCH_MODE;
                inputCallbacks.handleKeyDown(new KeyEvent(now, now, KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_FORWARD_DEL,
                        0, 0, KeyCharacterMap.VIRTUAL_KEYBOARD, 0, flags));
                inputCallbacks.handleKeyUp(new KeyEvent(now, now, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_FORWARD_DEL,
                        0, 0, KeyCharacterMap.VIRTUAL_KEYBOARD, 0, flags));
            }
        }

        @Override
        public CharSequence getTextBeforeCursor(int length, int flags) {
            return field.textBeforeCursor(length);
        }

        @Override
        public CharSequence getTextAfterCursor(int length, int flags) {
            return "";
        }

        @Override
        public CharSequence getSelectedText(int flags) {
            return null;
        }

        @Override
        public boolean setSelection(int start, int end) {
            // The host's caret stays where the typing left it.
            return false;
        }

        private boolean updated(boolean result) {
            InputMethodManager inputManager = (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
            if (inputManager != null) {
                inputManager.updateSelection(StreamView.this, field.cursor(), field.cursor(),
                        field.composingStart(), field.composingEnd());
            }
            return result;
        }
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        if (scalingMode == VideoScaling.STRETCH || videoWidth <= 0 || videoHeight <= 0) {
            super.onMeasure(widthMeasureSpec, heightMeasureSpec);
        } else {
            scaling.layout(scalingMode, alignment, videoWidth, videoHeight,
                    MeasureSpec.getSize(widthMeasureSpec), MeasureSpec.getSize(heightMeasureSpec));
            super.onMeasure(MeasureSpec.makeMeasureSpec(scaling.width(), MeasureSpec.EXACTLY),
                    MeasureSpec.makeMeasureSpec(scaling.height(), MeasureSpec.EXACTLY));
        }
        measurePaddedVideo();
    }

    // The video child keeps the top-left at this view's origin, so its padding hangs off the right
    // and bottom, where the clip (and on Android 13+ the SurfaceView's own surface crop) hides it.
    private void measurePaddedVideo() {
        android.view.View video = textureView != null ? textureView : surfaceView;
        if (video == null) return;
        int width = getMeasuredWidth(), height = getMeasuredHeight();
        if (paddedScaleX == 1 && paddedScaleY == 1) {
            video.setClipBounds(null);
            return;
        }
        video.measure(MeasureSpec.makeMeasureSpec(Math.round(width * paddedScaleX), MeasureSpec.EXACTLY),
                MeasureSpec.makeMeasureSpec(Math.round(height * paddedScaleY), MeasureSpec.EXACTLY));
        video.setClipBounds(new android.graphics.Rect(0, 0, width, height));
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
