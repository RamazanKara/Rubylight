package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.ColorFilter;
import android.graphics.LinearGradient;
import android.graphics.Outline;
import android.graphics.Paint;
import android.graphics.PixelFormat;
import android.graphics.Rect;
import android.graphics.RectF;
import android.graphics.Shader;
import android.graphics.drawable.Drawable;

import androidx.core.content.res.ResourcesCompat;
import androidx.core.graphics.ColorUtils;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;

/**
 * A flat control inside a glass panel: a tint over the panel, never a blur of its own. Its rim is lit from above as the panel's
 * is. Checked it takes the selected tint; hovered or pressed it lifts. Focused, the one ring of the app's glass stands on the
 * control's edge with a gap inside it, so a panel's clip can never cut it and nothing moves.
 */
final class GlassControlDrawable extends Drawable {
    enum Kind {
        /** A tile, a capsule, a round button. */
        CONTROL,
        /** The one red: Quit game. */
        DANGER,
        /** A row inside a group: no fill until it is touched or focused. */
        ROW,
        /** Rows standing together. Never takes state. */
        GROUP,
        /**
         * A key on a keyboard over the stream: its own opaque fill, so its label reads over any game whatever the panel
         * lets through. Selected (a held modifier) it takes the selected tint.
         */
        KEY
    }

    private final Kind kind;
    private final float radius;
    private final float ringWidth;
    private final float gapWidth;
    private final int fillColor;
    private final int checkedFill;
    private final int lift;
    private final int rimTop, rimMid, rimBottom, checkedRimTop;
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint rim = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint ring = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final RectF rect = new RectF();
    private boolean focused, checked, active;

    GlassControlDrawable(Context context, Kind kind, float radius) {
        this.kind = kind;
        this.radius = radius;
        android.content.res.Resources res = context.getResources();
        boolean highContrast = AppTheme.isHighContrast(context);
        ringWidth = res.getDimension(highContrast ? R.dimen.nj_hc_focus_ring_width : R.dimen.dialog_button_ring);
        gapWidth = res.getDimension(R.dimen.dialog_button_gap) - ringWidth / 2f;
        // A row can stand on a day sheet, where a white lift would not show.
        lift = kind == Kind.ROW ? MaterialColors.getColor(context, androidx.appcompat.R.attr.colorControlHighlight, 0)
                : ResourcesCompat.getColor(res, R.color.nj_focus_fill_lift, null);
        int controlRim = MaterialColors.getColor(context, R.attr.glassControlRim, 0);
        switch (kind) {
            case DANGER:
                fillColor = ResourcesCompat.getColor(res, R.color.nj_color_danger_fill, null);
                rimTop = rimMid = rimBottom = 0x26FFFFFF;
                break;
            case ROW:
                fillColor = 0;
                rimTop = rimMid = rimBottom = 0;
                break;
            case GROUP:
                fillColor = highContrast ? MaterialColors.getColor(context, R.attr.glassControlFill, 0)
                        : ResourcesCompat.getColor(res, R.color.nj_tint_group_fill, null);
                rimTop = highContrast ? controlRim : ResourcesCompat.getColor(res, R.color.nj_tint_group_rim_top, null);
                rimMid = highContrast ? controlRim : ResourcesCompat.getColor(res, R.color.nj_tint_group_rim_mid, null);
                rimBottom = highContrast ? controlRim : ResourcesCompat.getColor(res, R.color.nj_tint_group_rim_bottom, null);
                break;
            case KEY:
                fillColor = highContrast ? MaterialColors.getColor(context, R.attr.glassControlFill, 0)
                        : ResourcesCompat.getColor(res, R.color.nj_color_surface_high, null);
                rimTop = highContrast ? controlRim : ResourcesCompat.getColor(res, R.color.nj_tint_control_rim_top, null);
                rimMid = highContrast ? controlRim : ResourcesCompat.getColor(res, R.color.nj_tint_control_rim_mid, null);
                rimBottom = highContrast ? controlRim : ResourcesCompat.getColor(res, R.color.nj_tint_control_rim_bottom, null);
                break;
            default:
                fillColor = MaterialColors.getColor(context, R.attr.glassControlFill, 0);
                rimTop = highContrast ? controlRim : ResourcesCompat.getColor(res, R.color.nj_tint_control_rim_top, null);
                rimMid = highContrast ? controlRim : ResourcesCompat.getColor(res, R.color.nj_tint_control_rim_mid, null);
                rimBottom = highContrast ? controlRim : ResourcesCompat.getColor(res, R.color.nj_tint_control_rim_bottom, null);
                break;
        }
        int selected = MaterialColors.getColor(context, R.attr.glassSelectedFill, 0);
        // A key stays opaque when selected: the tint goes over its own fill.
        checkedFill = kind == Kind.KEY ? ColorUtils.compositeColors(selected, fillColor) : selected;
        checkedRimTop = MaterialColors.getColor(context, R.attr.glassSelectedRim, 0);
        rim.setStyle(Paint.Style.STROKE);
        rim.setStrokeWidth(Math.max(1f, res.getDisplayMetrics().density));
        ring.setStyle(Paint.Style.STROKE);
        ring.setStrokeWidth(ringWidth);
        ring.setColor(MaterialColors.getColor(context, kind == Kind.DANGER ? R.attr.glassRingOnMoon : R.attr.glassRing, 0));
    }

    private float corner(float width, float height, float inset) {
        return Math.max(0f, Math.min(radius, Math.min(width, height) / 2f) - inset);
    }

    private void applyRim() {
        Rect b = getBounds();
        int top = checked ? checkedRimTop : rimTop;
        if (b.isEmpty() || (top == 0 && rimMid == 0)) {
            rim.setShader(null);
            return;
        }
        rim.setShader(new LinearGradient(0, b.top, 0, b.bottom, new int[] {top, rimMid, rimBottom},
                new float[] {0f, 0.45f, 1f}, Shader.TileMode.CLAMP));
    }

    @Override
    protected void onBoundsChange(Rect bounds) {
        applyRim();
    }

    @Override
    public boolean isStateful() {
        return kind != Kind.GROUP;
    }

    /** Says the ring is the focus look: a view lays the system's grey square over a background that has none. */
    @Override
    public boolean hasFocusStateSpecified() {
        return kind != Kind.GROUP;
    }

    @Override
    protected boolean onStateChange(int[] state) {
        boolean nowFocused = false, nowChecked = false, nowActive = false;
        for (int s : state) {
            if (s == android.R.attr.state_focused) nowFocused = true;
            else if (s == android.R.attr.state_checked) nowChecked = true;
            // A key's selection is its modifier held: a button that is not checkable says it so.
            else if (s == android.R.attr.state_selected && kind == Kind.KEY) nowChecked = true;
            else if (s == android.R.attr.state_pressed || s == android.R.attr.state_hovered) nowActive = true;
        }
        if (nowFocused == focused && nowChecked == checked && nowActive == active) return false;
        boolean rimChanged = nowChecked != checked;
        focused = nowFocused;
        checked = nowChecked;
        active = nowActive;
        if (rimChanged) applyRim();
        return true;
    }

    float ringWidth() {
        return ringWidth;
    }

    Kind kind() {
        return kind;
    }

    int fillNow() {
        int base = checked ? checkedFill : fillColor;
        return focused || active ? ColorUtils.compositeColors(lift, base) : base;
    }

    boolean ringShown() {
        return focused;
    }

    @Override
    public void draw(Canvas canvas) {
        Rect b = getBounds();
        if (b.isEmpty()) return;
        float inset = focused ? ringWidth + gapWidth : 0f;
        int color = fillNow();
        if (color >>> 24 != 0) {
            fill.setColor(color);
            rect.set(b.left + inset, b.top + inset, b.right - inset, b.bottom - inset);
            float r = corner(b.width(), b.height(), inset);
            canvas.drawRoundRect(rect, r, r, fill);
        }
        if (focused) {
            float half = ringWidth / 2f;
            rect.set(b.left + half, b.top + half, b.right - half, b.bottom - half);
            float r = corner(b.width(), b.height(), half);
            canvas.drawRoundRect(rect, r, r, ring);
        } else if (rim.getShader() != null) {
            float half = rim.getStrokeWidth() / 2f;
            rect.set(b.left + half, b.top + half, b.right - half, b.bottom - half);
            float r = corner(b.width(), b.height(), half);
            canvas.drawRoundRect(rect, r, r, rim);
        }
    }

    @Override
    public void getOutline(Outline outline) {
        Rect b = getBounds();
        outline.setRoundRect(b, corner(b.width(), b.height(), 0f));
    }

    @Override
    public void setAlpha(int alpha) {
        fill.setAlpha(alpha);
    }

    /** A button style's tint would paint the control over: its colours are the design's. */
    @Override
    public void setColorFilter(ColorFilter filter) {
    }

    @Override
    public int getOpacity() {
        return PixelFormat.TRANSLUCENT;
    }
}
