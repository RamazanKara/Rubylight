package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.drawable.Drawable;
import android.text.TextPaint;
import android.text.TextUtils;
import android.view.Gravity;

import androidx.appcompat.content.res.AppCompatResources;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.limelight.R;

/** A status row in a stream menu group: its name at the start, its value in the quiet colour at the end, a chevron after it. */
public final class MenuRow extends MaterialButton {
    private final Drawable chevron;
    private final TextPaint valuePaint = new TextPaint(Paint.ANTI_ALIAS_FLAG);
    private final int chevronSize;
    private final int gap;
    private CharSequence value;

    MenuRow(Context context, CharSequence label, CharSequence value) {
        super(context, null, androidx.appcompat.R.attr.borderlessButtonStyle);
        android.content.res.Resources res = context.getResources();
        setText(label);
        setAllCaps(false);
        setTextAppearance(context, R.style.TextAppearance_Nightjar_Label);
        setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0));
        setGravity(Gravity.START | Gravity.CENTER_VERTICAL);
        setSingleLine(true);
        setEllipsize(TextUtils.TruncateAt.END);
        setInsetTop(0);
        setInsetBottom(0);
        int height = res.getDimensionPixelSize(R.dimen.stream_menu_row_height);
        setMinHeight(height);
        setMinimumHeight(height);
        gap = Math.round(res.getDisplayMetrics().density * 8);
        chevronSize = Math.round(res.getDisplayMetrics().density * 18);
        setPaddingRelative(gap * 2, 0, gap * 2 + chevronSize, 0);
        setBackground(new GlassControlDrawable(context, GlassControlDrawable.Kind.ROW, res.getDimension(R.dimen.nj_radius_control)));
        chevron = AppCompatResources.getDrawable(context, R.drawable.settings_chevron);
        valuePaint.setTextSize(res.getDimension(R.dimen.stream_menu_row_value));
        valuePaint.setColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurfaceVariant, 0));
        valuePaint.setTypeface(getTypeface());
        setValue(value);
    }

    public void setValue(CharSequence text) {
        value = text;
        CharSequence name = getText();
        setContentDescription(text == null || text.length() == 0 ? name : name + ", " + text);
        invalidate();
    }

    public CharSequence value() {
        return value;
    }

    @Override
    protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
        int top = (getHeight() - chevronSize) / 2;
        int inset = gap;
        int chevronLeft = rtl ? inset : getWidth() - inset - chevronSize;
        chevron.setAutoMirrored(true);
        chevron.setLayoutDirection(getLayoutDirection());
        chevron.setBounds(chevronLeft, top, chevronLeft + chevronSize, top + chevronSize);
        chevron.draw(canvas);
        if (value == null || value.length() == 0) return;
        float label = getPaint().measureText(getText().toString());
        float room = getWidth() - 2f * inset - chevronSize - gap - gap * 2 - label - gap * 2;
        if (room <= 0) return;
        CharSequence fitted = TextUtils.ellipsize(value, valuePaint, room, TextUtils.TruncateAt.END);
        float width = valuePaint.measureText(fitted, 0, fitted.length());
        float x = rtl ? inset + chevronSize + gap : getWidth() - inset - chevronSize - gap - width;
        Paint.FontMetrics metrics = valuePaint.getFontMetrics();
        canvas.drawText(fitted, 0, fitted.length(), x, getHeight() / 2f - (metrics.ascent + metrics.descent) / 2f, valuePaint);
    }
}
