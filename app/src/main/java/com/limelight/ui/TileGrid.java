package com.limelight.ui;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.view.View;
import android.view.ViewGroup;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.limelight.R;

/**
 * The stream menu's grid: square-ish tiles in equal columns, every tile in a row as tall as the
 * tallest. Four columns, fewer only when a tile would be too narrow for its label (a narrow sheet
 * or a large font).
 */
public final class TileGrid extends ViewGroup {
    public static final int COLUMNS = 4;
    private final int gap;
    private final int maxColumns;
    private final int minTile;
    private int columns = COLUMNS;

    public TileGrid(Context context, int gap) {
        this(context, gap, COLUMNS, 68);
    }

    /** {@code maxColumns} tiles to a row while each stays at least {@code minTileDp} wide at this font size. */
    public TileGrid(Context context, int gap, int maxColumns, int minTileDp) {
        super(context);
        this.gap = gap;
        this.maxColumns = maxColumns;
        float scale = context.getResources().getConfiguration().fontScale;
        minTile = Math.round(minTileDp * Math.max(1f, scale) * context.getResources().getDisplayMetrics().density);
    }

    /** How many tiles share a row now. */
    public int columns() {
        return columns;
    }

    @Override
    protected void onMeasure(int widthSpec, int heightSpec) {
        int width = MeasureSpec.getSize(widthSpec);
        int room = width - getPaddingLeft() - getPaddingRight();
        columns = maxColumns;
        while (columns > 2 && (room - gap * (columns - 1)) / columns < minTile) columns--;
        int tile = Math.max(0, (room - gap * (columns - 1)) / columns);
        int count = getChildCount();
        int height = getPaddingTop() + getPaddingBottom();
        for (int start = 0; start < count; start += columns) {
            int tallest = 0;
            for (int i = start; i < Math.min(count, start + columns); i++) {
                View child = getChildAt(i);
                child.measure(MeasureSpec.makeMeasureSpec(tile, MeasureSpec.EXACTLY), MeasureSpec.makeMeasureSpec(0, MeasureSpec.UNSPECIFIED));
                tallest = Math.max(tallest, Math.max(child.getMeasuredHeight(), child.getMinimumHeight()));
            }
            for (int i = start; i < Math.min(count, start + columns); i++) {
                getChildAt(i).measure(MeasureSpec.makeMeasureSpec(tile, MeasureSpec.EXACTLY), MeasureSpec.makeMeasureSpec(tallest, MeasureSpec.EXACTLY));
            }
            height += tallest + (start > 0 ? gap : 0);
        }
        setMeasuredDimension(width, height);
    }

    @Override
    protected void onLayout(boolean changed, int left, int top, int right, int bottom) {
        boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
        int y = getPaddingTop();
        int count = getChildCount();
        for (int start = 0; start < count; start += columns) {
            int tallest = 0;
            for (int i = start; i < Math.min(count, start + columns); i++) {
                View child = getChildAt(i);
                int x = (i - start) * (child.getMeasuredWidth() + gap);
                int childLeft = rtl ? right - left - getPaddingRight() - x - child.getMeasuredWidth() : getPaddingLeft() + x;
                child.layout(childLeft, y, childLeft + child.getMeasuredWidth(), y + child.getMeasuredHeight());
                tallest = Math.max(tallest, child.getMeasuredHeight());
            }
            y += tallest + gap;
        }
    }

    /**
     * A tile: its icon over a label of up to two lines, a quieter line under it (a shortcut's keys),
     * a small switch in the top end corner while it has a state, and the controller button that runs
     * it in the top start corner. On, it takes the amber container colour.
     */
    public static final class Tile extends MaterialButton {
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final RectF box = new RectF();
        private final float dp;
        private String hotkey;
        private boolean hotkeyAtEnd;
        private boolean capInLabelColour;

        public Tile(Context context) {
            this(context, com.google.android.material.R.attr.materialButtonOutlinedStyle);
        }

        Tile(Context context, int style) {
            super(context, null, style);
            dp = context.getResources().getDisplayMetrics().density;
        }

        /** The controller button's cap, {@code "Y"}, or null. */
        public void setHotkey(String hotkey) {
            this.hotkey = hotkey;
            invalidate();
        }

        /** The cap at the end of a wide row, centred on its height, instead of in the top corner. */
        void setHotkeyAtEnd(boolean atEnd) {
            hotkeyAtEnd = atEnd;
            invalidate();
        }

        /** The cap in the top corner in the label's colour, as on a filled button, instead of the quiet one. */
        void setCapInLabelColour(boolean labelColour) {
            capInLabelColour = labelColour;
            invalidate();
        }

        public String hotkey() {
            return hotkey;
        }

        /** A switch's state, or null for an action. A switch flips {@code isChecked()} on click before its listener runs. */
        public void setOn(Boolean on) {
            setCheckable(on != null);
            setChecked(Boolean.TRUE.equals(on));
            invalidate();
        }

        @Override
        protected void onDraw(Canvas canvas) {
            super.onDraw(canvas);
            int quiet = MaterialColors.getColor(this, com.google.android.material.R.attr.colorOnSurfaceVariant);
            if (isCheckable()) {
                // A small switch: amber track with the knob at the end while on, an outline with it at the start while off.
                boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
                float w = 22 * dp, h = 12 * dp, inset = 8 * dp;
                float left = rtl ? inset : getWidth() - inset - w;
                box.set(left, inset, left + w, inset + h);
                boolean on = isChecked();
                boolean knobRight = on != rtl;
                if (on) {
                    paint.setStyle(Paint.Style.FILL);
                    paint.setColor(MaterialColors.getColor(this, androidx.appcompat.R.attr.colorPrimary));
                    canvas.drawRoundRect(box, h / 2, h / 2, paint);
                    paint.setColor(MaterialColors.getColor(this, com.google.android.material.R.attr.colorOnPrimary));
                } else {
                    paint.setStyle(Paint.Style.STROKE);
                    paint.setStrokeWidth(1.2f * dp);
                    paint.setColor(quiet);
                    canvas.drawRoundRect(box, h / 2, h / 2, paint);
                    paint.setColor(quiet);
                }
                paint.setStyle(Paint.Style.FILL);
                float cx = knobRight ? box.right - h / 2 : box.left + h / 2;
                canvas.drawCircle(cx, box.centerY(), (on ? 4 : 3.5f) * dp, paint);
            }
            if (hotkey != null) {
                paint.setStyle(Paint.Style.FILL);
                paint.setTextSize(11 * dp * getResources().getConfiguration().fontScale);
                paint.setFakeBoldText(true);
                float text = paint.measureText(hotkey);
                boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
                float w = text + 10 * dp, h = paint.getTextSize() + 5 * dp, inset = (hotkeyAtEnd ? 16 : 8) * dp;
                float left = rtl != hotkeyAtEnd ? getWidth() - inset - w : inset;
                float top = hotkeyAtEnd ? (getHeight() - h) / 2 : 6 * dp;
                box.set(left, top, left + w, top + h);
                paint.setStyle(Paint.Style.STROKE);
                paint.setStrokeWidth(dp);
                paint.setColor(hotkeyAtEnd || capInLabelColour ? getCurrentTextColor() : quiet);
                canvas.drawRoundRect(box, 4 * dp, 4 * dp, paint);
                paint.setStyle(Paint.Style.FILL);
                Paint.FontMetrics metrics = paint.getFontMetrics();
                canvas.drawText(hotkey, box.centerX() - text / 2, box.centerY() - (metrics.ascent + metrics.descent) / 2, paint);
                paint.setFakeBoldText(false);
            }
        }
    }

    /** A tile in this grid, styled like the sheet's toggles: outlined, filled while on. */
    public Tile addTile(int icon, CharSequence label, CharSequence detail, Boolean on, String hotkey) {
        Context context = getContext();
        Tile tile = new Tile(context);
        int spacing = gap;
        tile.setText(detail == null ? label : withDetail(context, label, detail));
        tile.setAllCaps(false);
        tile.setTextAppearance(context, R.style.TextAppearance_Nightjar_Label);
        tile.setSingleLine(false);
        tile.setMaxLines(detail == null ? 2 : 3);
        tile.setEllipsize(android.text.TextUtils.TruncateAt.END);
        tile.setIconResource(icon);
        tile.setIconGravity(MaterialButton.ICON_GRAVITY_TOP);
        tile.setIconSize(spacing * 3);
        tile.setIconPadding(spacing / 2);
        tile.setMinHeight(spacing * 12);
        // A rounded square, not the sheet's pill buttons: the corner caps sit inside its outline.
        tile.setCornerRadius(spacing * 3 / 2);
        tile.setInsetTop(0);
        tile.setInsetBottom(0);
        tile.setPaddingRelative(spacing / 2, spacing * 3, spacing / 2, spacing);
        int[][] states = {new int[] {android.R.attr.state_checked}, new int[] {}};
        android.content.res.ColorStateList foreground = new android.content.res.ColorStateList(states, new int[] {
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnPrimaryContainer, 0),
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0)});
        tile.setTextColor(foreground);
        tile.setIconTint(foreground);
        tile.setBackgroundTintList(new android.content.res.ColorStateList(states, new int[] {
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorPrimaryContainer, 0),
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorSurfaceContainerHigh, 0)}));
        tile.setOn(on);
        tile.setHotkey(hotkey);
        // On a TV, the sheet's toggles' focus look: the amber ring, growing a little under it.
        if ((context.getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_TYPE_MASK)
                == android.content.res.Configuration.UI_MODE_TYPE_TELEVISION) {
            tile.setStrokeColor(androidx.appcompat.content.res.AppCompatResources.getColorStateList(context, R.color.card_stroke));
            tile.setStateListAnimator(Motion.controlFocus(context));
        }
        if (hotkey != null) {
            tile.setContentDescription(label + (detail == null ? "" : ", " + detail) + ", " + hotkey);
        } else if (detail != null) {
            tile.setContentDescription(label + ", " + detail);
        }
        addView(tile);
        return tile;
    }

    private static CharSequence withDetail(Context context, CharSequence label, CharSequence detail) {
        android.text.SpannableStringBuilder text = new android.text.SpannableStringBuilder(label).append('\n');
        int start = text.length();
        text.append(detail);
        text.setSpan(new android.text.style.TextAppearanceSpan(context, R.style.TextAppearance_Nightjar_Supporting),
                start, text.length(), 0);
        text.setSpan(new android.text.style.ForegroundColorSpan(MaterialColors.getColor(context,
                com.google.android.material.R.attr.colorOnSurfaceVariant, 0)), start, text.length(), 0);
        return text;
    }
}
