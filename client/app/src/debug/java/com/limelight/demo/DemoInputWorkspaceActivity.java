package com.limelight.demo;

import android.app.Activity;
import android.content.Context;
import android.content.res.Configuration;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.os.Bundle;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowManager;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.ui.DesktopControlsPanel;

/** Hostless review surface: the production controls operate a small local, editable desktop. */
public final class DemoInputWorkspaceActivity extends Activity {
    private DesktopControlsPanel controls;
    private LocalDesktop desktop;

    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override protected void onCreate(Bundle state) {
        setTheme(R.style.StreamTheme);
        super.onCreate(state);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN);
        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_HIDDEN |
                WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);

        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(0xff101619);
        root.setFitsSystemWindows(true);
        LinearLayout header = new LinearLayout(this);
        header.setGravity(Gravity.CENTER_VERTICAL);
        header.setPadding(dp(16), 0, dp(8), 0);
        TextView title = new TextView(this);
        title.setText("Nightjar · Local preview");
        title.setTextColor(0xffe6eaec);
        title.setTextSize(15);
        title.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
        header.addView(title, new LinearLayout.LayoutParams(0, dp(48), 1));
        title.setGravity(Gravity.CENTER_VERTICAL);
        MaterialButton reopen = new MaterialButton(this, null, androidx.appcompat.R.attr.borderlessButtonStyle);
        reopen.setText("Controls");
        reopen.setAllCaps(false);
        reopen.setContentDescription("Show desktop controls");
        reopen.setOnClickListener(view -> controls.setVisibility(View.VISIBLE));
        header.addView(reopen, new LinearLayout.LayoutParams(-2, dp(48)));
        root.addView(header, new LinearLayout.LayoutParams(-1, dp(48)));

        desktop = new LocalDesktop(this);
        root.addView(desktop, new LinearLayout.LayoutParams(-1, 0, 1));
        controls = new DesktopControlsPanel(this, new DesktopControlsPanel.Sink() {
            @Override public void sendKeys(int[] keys) { desktop.sendKeys(keys); }
            @Override public void move(float dx, float dy) { desktop.movePointer(dx, dy); }
            @Override public void scroll(float vertical, float horizontal) { desktop.scrollNote(vertical, horizontal); }
            @Override public void mouseButton(int button, boolean down) { desktop.mouseButton(button, down); }
        }, this::showSystemKeyboard, () -> {
            controls.releaseInputs();
            controls.setVisibility(View.GONE);
        });
        root.addView(controls, new LinearLayout.LayoutParams(-1, -2));
        setContentView(root);

        DesktopControlsPanel.Mode initial = getResources().getConfiguration().orientation == Configuration.ORIENTATION_LANDSCAPE ?
                DesktopControlsPanel.Mode.COMBINED : DesktopControlsPanel.Mode.KEYBOARD;
        String mode = state != null ? state.getString("mode") : getIntent().getStringExtra("mode");
        if (mode != null) {
            try { initial = DesktopControlsPanel.Mode.valueOf(mode.toUpperCase(java.util.Locale.ROOT)); }
            catch (IllegalArgumentException ignored) { /* Keep the orientation's default. */ }
        }
        controls.setMode(initial);
        if (state != null) {
            desktop.note.setText(state.getString("note", ""));
            controls.setVisibility(state.getBoolean("controls", true) ? View.VISIBLE : View.GONE);
        }
        desktop.note.setSelection(desktop.note.length());
    }

    public DesktopControlsPanel controls() { return controls; }

    private void showSystemKeyboard() {
        desktop.note.setShowSoftInputOnFocus(true);
        desktop.note.requestFocus();
        ((InputMethodManager) getSystemService(INPUT_METHOD_SERVICE))
                .showSoftInput(desktop.note, InputMethodManager.SHOW_IMPLICIT);
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        state.putString("mode", controls.getMode().name());
        state.putString("note", desktop.note.getText().toString());
        state.putBoolean("controls", controls.getVisibility() == View.VISIBLE);
        super.onSaveInstanceState(state);
    }

    @Override protected void onPause() {
        if (controls != null) controls.releaseInputs();
        super.onPause();
    }

    @Override public void onWindowFocusChanged(boolean focused) {
        super.onWindowFocusChanged(focused);
        if (!focused && controls != null) controls.releaseInputs();
    }

    private int dp(float value) { return Math.round(value * getResources().getDisplayMetrics().density); }

    private final class LocalDesktop extends FrameLayout {
        final EditText note;
        private final LinearLayout window;
        private final TextView feedback;
        private final Pointer cursor;
        private float pointerX, pointerY, dragX, dragY;
        private boolean positioned, dragging;

        LocalDesktop(Context context) {
            super(context);
            setBackgroundColor(0xff19282c);
            setClipChildren(true);
            setContentDescription("Local desktop preview. No PC is connected.");
            window = new LinearLayout(context);
            window.setOrientation(LinearLayout.VERTICAL);
            GradientDrawable background = new GradientDrawable();
            background.setColor(0xff202b30);
            background.setCornerRadius(dp(12));
            background.setStroke(dp(1), 0xff405056);
            window.setBackground(background);
            window.setClipToOutline(true);
            TextView caption = new TextView(context);
            caption.setText("Notes");
            caption.setTextSize(14);
            caption.setTextColor(0xffe5eaec);
            caption.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
            caption.setGravity(Gravity.CENTER_VERTICAL);
            caption.setPadding(dp(16), 0, dp(16), 0);
            caption.setBackgroundColor(0xff2b3a40);
            caption.setContentDescription("Notes window title. Drag with the left mouse button.");
            window.addView(caption, new LinearLayout.LayoutParams(-1, dp(38)));
            note = new EditText(context);
            note.setText("Try typing here…\n\nMove the pointer with the trackpad. Hold the left mouse button to drag this Notes window.\n\nChanges stay on this device.");
            note.setTextColor(0xffdce4e7);
            note.setHintTextColor(0xff95a7ae);
            note.setTextSize(16);
            note.setGravity(Gravity.TOP | Gravity.START);
            note.setPadding(dp(16), dp(12), dp(16), dp(12));
            note.setBackgroundColor(android.graphics.Color.TRANSPARENT);
            note.setSingleLine(false);
            note.setInputType(android.text.InputType.TYPE_CLASS_TEXT | android.text.InputType.TYPE_TEXT_FLAG_MULTI_LINE |
                    android.text.InputType.TYPE_TEXT_FLAG_CAP_SENTENCES);
            note.setShowSoftInputOnFocus(false);
            note.setContentDescription("Editable preview note");
            window.addView(note, new LinearLayout.LayoutParams(-1, 0, 1));
            FrameLayout.LayoutParams noteParams = new FrameLayout.LayoutParams(-1, -1);
            noteParams.setMargins(dp(24), dp(16), dp(24), dp(40));
            addView(window, noteParams);
            feedback = new TextView(context);
            feedback.setText("No PC connected · Your input stays in this preview");
            feedback.setTextColor(0xffa9bcc3);
            feedback.setTextSize(12);
            feedback.setGravity(Gravity.CENTER_VERTICAL);
            feedback.setSingleLine(true);
            feedback.setEllipsize(android.text.TextUtils.TruncateAt.END);
            FrameLayout.LayoutParams status = new FrameLayout.LayoutParams(-1, dp(32), Gravity.BOTTOM);
            status.setMargins(dp(24), 0, dp(24), dp(4));
            addView(feedback, status);
            cursor = new Pointer(context);
            addView(cursor, new FrameLayout.LayoutParams(dp(22), dp(28)));
            caption.setOnTouchListener(new View.OnTouchListener() {
                float lastX, lastY;
                @Override public boolean onTouch(View view, MotionEvent event) {
                    if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                        lastX = event.getRawX(); lastY = event.getRawY(); return true;
                    }
                    if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
                        moveWindow(event.getRawX() - lastX, event.getRawY() - lastY);
                        lastX = event.getRawX(); lastY = event.getRawY(); return true;
                    }
                    if (event.getActionMasked() == MotionEvent.ACTION_UP) view.performClick();
                    return true;
                }
            });
        }

        @Override protected void onMeasure(int widthSpec, int heightSpec) {
            int available = MeasureSpec.getSize(widthSpec);
            FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) window.getLayoutParams();
            params.width = Math.max(dp(100), Math.min(available - dp(48), dp(760)));
            params.leftMargin = params.rightMargin = Math.max(0, (available - params.width) / 2);
            super.onMeasure(widthSpec, heightSpec);
        }

        @Override protected void onSizeChanged(int width, int height, int oldWidth, int oldHeight) {
            super.onSizeChanged(width, height, oldWidth, oldHeight);
            if (!positioned) { pointerX = width * 0.7f; pointerY = height * 0.35f; positioned = true; }
            pointerX = Math.max(0, Math.min(pointerX, width - dp(22)));
            pointerY = Math.max(0, Math.min(pointerY, height - dp(28)));
            placePointer();
        }

        void sendKeys(int[] keys) {
            if (keys == null || keys.length == 0) return;
            StringBuilder chord = new StringBuilder();
            int meta = 0;
            for (int key : keys) {
                if (chord.length() > 0) chord.append(" + ");
                chord.append(KeyEvent.keyCodeToString(key).replace("KEYCODE_", "").replace("_LEFT", ""));
                if (key == KeyEvent.KEYCODE_CTRL_LEFT || key == KeyEvent.KEYCODE_CTRL_RIGHT) meta |= KeyEvent.META_CTRL_ON;
                if (key == KeyEvent.KEYCODE_ALT_LEFT || key == KeyEvent.KEYCODE_ALT_RIGHT) meta |= KeyEvent.META_ALT_ON;
                if (key == KeyEvent.KEYCODE_SHIFT_LEFT || key == KeyEvent.KEYCODE_SHIFT_RIGHT) meta |= KeyEvent.META_SHIFT_ON;
                if (key == KeyEvent.KEYCODE_META_LEFT || key == KeyEvent.KEYCODE_META_RIGHT) meta |= KeyEvent.META_META_ON;
            }
            feedback.setText(chord);
            note.requestFocus();
            long now = android.os.SystemClock.uptimeMillis();
            int key = keys[keys.length - 1];
            // Keep system/window shortcuts local; this preview has only the Notes editor.
            if ((meta & (KeyEvent.META_ALT_ON | KeyEvent.META_META_ON)) == 0 && !KeyEvent.isModifierKey(key)) {
                KeyEvent keyDown = new KeyEvent(now, now, KeyEvent.ACTION_DOWN, key, 0, meta);
                if ((meta & KeyEvent.META_CTRL_ON) != 0) {
                    note.onKeyShortcut(key, keyDown);
                    return;
                }
                note.dispatchKeyEvent(keyDown);
                note.dispatchKeyEvent(new KeyEvent(now, now, KeyEvent.ACTION_UP, key, 0, meta));
            }
        }

        void movePointer(float dx, float dy) {
            float density = getResources().getDisplayMetrics().density;
            dx *= density; dy *= density;
            pointerX = Math.max(0, Math.min(pointerX + dx, getWidth() - dp(22)));
            pointerY = Math.max(0, Math.min(pointerY + dy, getHeight() - dp(28)));
            if (dragging) {
                moveWindow(pointerX - dragX, pointerY - dragY);
                dragX = pointerX; dragY = pointerY;
            }
            placePointer();
        }

        void scrollNote(float vertical, float horizontal) {
            int maxScroll = note.getLayout() == null ? 0 : Math.max(0,
                    note.getLayout().getHeight() - note.getHeight() + note.getCompoundPaddingTop() + note.getCompoundPaddingBottom());
            note.scrollTo(0, Math.max(0, Math.min(maxScroll, note.getScrollY() - Math.round(vertical * dp(48) / 120f))));
            feedback.setText("Scroll " + Math.round(vertical) + (horizontal != 0 ? " · sideways " + Math.round(horizontal) : ""));
        }

        void mouseButton(int button, boolean down) {
            String name = button == 1 ? "Left" : button == 2 ? "Middle" : button == 3 ? "Right" : button == 4 ? "Back" : "Forward";
            feedback.setText(name + " mouse · " + (down ? "held" : "released"));
            if (button != 1) return;
            if (down) {
                dragging = pointerX >= window.getX() && pointerX <= window.getX() + window.getWidth() &&
                        pointerY >= window.getY() && pointerY <= window.getY() + dp(38);
                dragX = pointerX; dragY = pointerY;
                if (!dragging) note.requestFocus();
            } else dragging = false;
        }

        private void moveWindow(float dx, float dy) {
            float minX = -window.getLeft() + dp(8), maxX = getWidth() - window.getRight() - dp(8);
            float minY = -window.getTop() + dp(4), maxY = getHeight() - window.getTop() - dp(50);
            window.setTranslationX(Math.max(minX, Math.min(maxX, window.getTranslationX() + dx)));
            window.setTranslationY(Math.max(minY, Math.min(maxY, window.getTranslationY() + dy)));
        }

        private void placePointer() { cursor.setTranslationX(pointerX); cursor.setTranslationY(pointerY); }
    }

    private final class Pointer extends View {
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Path arrow = new Path();
        Pointer(Context context) { super(context); setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO); }
        @Override protected void onDraw(Canvas canvas) {
            super.onDraw(canvas);
            arrow.reset();
            arrow.moveTo(dp(2), dp(2)); arrow.lineTo(dp(2), dp(23)); arrow.lineTo(dp(7), dp(18));
            arrow.lineTo(dp(11), dp(26)); arrow.lineTo(dp(15), dp(24)); arrow.lineTo(dp(11), dp(16));
            arrow.lineTo(dp(19), dp(16)); arrow.close();
            paint.setStyle(Paint.Style.FILL); paint.setColor(0xfff7fafb); canvas.drawPath(arrow, paint);
            paint.setStyle(Paint.Style.STROKE); paint.setStrokeWidth(dp(1)); paint.setColor(0xff172125); canvas.drawPath(arrow, paint);
        }
    }
}
