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
    private final Paint capPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final android.graphics.RectF capBox = new android.graphics.RectF();
    private String hotkey;
    private boolean capShown;

    MenuRow(Context context, CharSequence label, CharSequence value) {
        this(context, (android.util.AttributeSet) null);
        setText(label);
        setValue(value);
    }

    public MenuRow(Context context, android.util.AttributeSet attrs) {
        super(context, attrs, androidx.appcompat.R.attr.borderlessButtonStyle);
        android.content.res.Resources res = context.getResources();
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
        capPaint.setTextSize(res.getDimension(R.dimen.stream_tile_cap_text));
        capPaint.setFakeBoldText(true);
    }

    /**
     * The controller button that opens the row ({@code "Y"}), drawn before the chevron as a way out's cap is and said with its
     * name while {@code shown}; a remote has no face buttons, so then it is neither drawn nor said. Null for none.
     */
    void setHotkey(String hotkey, boolean shown) {
        this.hotkey = hotkey;
        capShown = shown;
        // The name ends before the cap, as before the chevron.
        int end = gap * 2 + chevronSize + (capShown() ? Math.round(capWidth()) + gap : 0);
        setPaddingRelative(getPaddingStart(), getPaddingTop(), end, getPaddingBottom());
        setValue(value);
    }

    String hotkey() {
        return hotkey;
    }

    boolean capShown() {
        return hotkey != null && capShown;
    }

    private float capWidth() {
        if (!capShown()) return 0;
        float text = capPaint.measureText(hotkey);
        return Math.max(text + capPaint.getTextSize() * 0.8f, capPaint.getTextSize() * 1.6f);
    }

    /** Its name over a quieter line, for a value too long to stand beside the name. */
    void setDetail(CharSequence label, CharSequence detail) {
        android.text.SpannableStringBuilder text = new android.text.SpannableStringBuilder(label).append('\n');
        int start = text.length();
        text.append(detail);
        int end = text.length();
        text.setSpan(new android.text.style.AbsoluteSizeSpan(Math.round(valuePaint.getTextSize())), start, end, android.text.Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        text.setSpan(new android.text.style.ForegroundColorSpan(valuePaint.getColor()), start, end, android.text.Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
        setSingleLine(false);
        setMaxLines(2);
        setPaddingRelative(gap * 2, gap, gap * 2 + chevronSize, gap);
        setText(text);
        value = null;
        setContentDescription(label + ", " + detail);
    }

    public void setValue(CharSequence text) {
        value = text;
        CharSequence name = getText();
        CharSequence said = text == null || text.length() == 0 ? name : name + ", " + text;
        setContentDescription(capShown() ? said + ", " + hotkey : said);
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
        androidx.core.graphics.drawable.DrawableCompat.setLayoutDirection(chevron, getLayoutDirection());
        chevron.setBounds(chevronLeft, top, chevronLeft + chevronSize, top + chevronSize);
        chevron.draw(canvas);
        float cap = capWidth();
        if (cap > 0) {
            float density = getResources().getDisplayMetrics().density;
            float h = capPaint.getTextSize() * 1.45f;
            float left = rtl ? inset + chevronSize + gap : getWidth() - inset - chevronSize - gap - cap;
            capBox.set(left, (getHeight() - h) / 2, left + cap, (getHeight() + h) / 2);
            capPaint.setColor(getCurrentTextColor());
            capPaint.setStyle(Paint.Style.STROKE);
            capPaint.setStrokeWidth(density);
            canvas.drawRoundRect(capBox, 3 * density, 3 * density, capPaint);
            capPaint.setStyle(Paint.Style.FILL);
            float text = capPaint.measureText(hotkey);
            Paint.FontMetrics capMetrics = capPaint.getFontMetrics();
            canvas.drawText(hotkey, capBox.centerX() - text / 2, capBox.centerY() - (capMetrics.ascent + capMetrics.descent) / 2, capPaint);
            cap += gap;
        }
        if (value == null || value.length() == 0) return;
        float label = getPaint().measureText(getText().toString());
        float room = getWidth() - 2f * inset - chevronSize - gap - cap - gap * 2 - label - gap * 2;
        if (room <= 0) return;
        CharSequence fitted = TextUtils.ellipsize(value, valuePaint, room, TextUtils.TruncateAt.END);
        float width = valuePaint.measureText(fitted, 0, fitted.length());
        float x = rtl ? inset + chevronSize + gap + cap : getWidth() - inset - chevronSize - gap - cap - width;
        Paint.FontMetrics metrics = valuePaint.getFontMetrics();
        canvas.drawText(fitted, 0, fitted.length(), x, getHeight() / 2f - (metrics.ascent + metrics.descent) / 2f, valuePaint);
    }
}
