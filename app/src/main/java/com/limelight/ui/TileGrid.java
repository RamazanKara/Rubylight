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
 * The stream menu's grid, compact like desktop Nightjar's: tiles in equal columns, every tile in a row as tall as
 * the tallest. As many columns as fit while each tile stays wide enough for its label (six at most on a handheld,
 * four on a TV), at least two.
 */
public final class TileGrid extends ViewGroup {
    /** Desktop Nightjar's columns; a TV's grid keeps them. */
    public static final int COLUMNS = 4;
    private final int gap;
    private final int maxColumns;
    private final int minTile;
    private int columns = COLUMNS;
    private boolean capsShown = true;

    /** The stream menu's grid, sized by {@code dimens_stream_menu} for this device. */
    public TileGrid(Context context) {
        this(context, context.getResources().getDimensionPixelSize(R.dimen.stream_tile_gap),
                context.getResources().getInteger(R.integer.stream_tile_columns),
                Math.round(context.getResources().getDimension(R.dimen.stream_tile_min_width) / context.getResources().getDisplayMetrics().density));
    }

    /** {@code maxColumns} tiles to a row while each stays at least {@code minTileDp} wide at this font size. */
    public TileGrid(Context context, int gap, int maxColumns, int minTileDp) {
        super(context);
        this.gap = gap;
        this.maxColumns = maxColumns;
        float scale = context.getResources().getConfiguration().fontScale;
        minTile = Math.round(minTileDp * Math.max(1f, scale) * context.getResources().getDisplayMetrics().density);
    }

    /** The gap between tiles, in pixels. */
    public int gap() {
        return gap;
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
     * A tile: its icon over a label of up to two lines, a quieter line under it (a shortcut's keys), and the controller
     * button that runs it in the top start corner. On, it takes the selected tint and its ink; focused, the glass ring.
     * Sized by {@code dimens_stream_menu}.
     */
    public static final class Tile extends MaterialButton {
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final RectF box = new RectF();
        private final float dp;
        private final float capText, cornerInset;
        // A remote has no Y or RB: the caps only show while a controller is there to press them.
        private boolean capShown = true;
        private String hotkey;
        private boolean hotkeyAtEnd;
        private boolean isSwitch;

        public Tile(Context context) {
            this(context, com.google.android.material.R.attr.materialButtonOutlinedStyle);
        }

        Tile(Context context, int style) {
            super(context, null, style);
            android.content.res.Resources res = context.getResources();
            dp = res.getDisplayMetrics().density;
            capText = res.getDimension(R.dimen.stream_tile_cap_text);
            cornerInset = res.getDimension(R.dimen.stream_tile_corner_inset);
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

        /** Whether the cap is drawn; its button works either way. */
        void setCapShown(boolean shown) {
            capShown = shown;
            invalidate();
        }

        boolean capShown() {
            return capShown;
        }

        public String hotkey() {
            return hotkey;
        }

        // MaterialButton drops a change of checkable once a background of the app's own replaces its shape.
        @Override
        public void setCheckable(boolean checkable) {
            isSwitch = checkable;
            refreshDrawableState();
        }

        @Override
        public boolean isCheckable() {
            return isSwitch;
        }

        /** A switch's state, or null for an action. A switch flips {@code isChecked()} on click before its listener runs. */
        public void setOn(Boolean on) {
            setCheckable(on != null);
            setChecked(Boolean.TRUE.equals(on));
            invalidate();
        }

        /**
         * Words break at a syllable only when one alone is wider than the tile (Bildschirmtasten), so PC commands wraps
         * between its words as desktop Nightjar's does, not inside one.
         */
        @Override
        protected void onMeasure(int widthSpec, int heightSpec) {
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.M
                    && MeasureSpec.getMode(widthSpec) != MeasureSpec.UNSPECIFIED) {
                float room = MeasureSpec.getSize(widthSpec) - getCompoundPaddingLeft() - getCompoundPaddingRight();
                CharSequence text = getText();
                int end = text.toString().indexOf('\n');
                boolean fits = true;
                for (String word : text.subSequence(0, end < 0 ? text.length() : end).toString().split("\\s+")) {
                    fits &= getPaint().measureText(word) <= room;
                }
                int frequency = fits ? android.text.Layout.HYPHENATION_FREQUENCY_NONE : android.text.Layout.HYPHENATION_FREQUENCY_FULL;
                if (getHyphenationFrequency() != frequency) setHyphenationFrequency(frequency);
            }
            super.onMeasure(widthSpec, heightSpec);
        }

        @Override
        protected void onDraw(Canvas canvas) {
            super.onDraw(canvas);
            int quiet = MaterialColors.getColor(this, com.google.android.material.R.attr.colorOnSurfaceVariant);
            boolean rtl = getLayoutDirection() == LAYOUT_DIRECTION_RTL;
            if (hotkey != null && capShown) {
                paint.setStyle(Paint.Style.FILL);
                paint.setTextSize(capText);
                paint.setFakeBoldText(true);
                float text = paint.measureText(hotkey);
                float w = Math.max(text + capText * 0.8f, capText * 1.6f), h = capText * 1.45f;
                float inset = hotkeyAtEnd ? 2 * cornerInset + 4 * dp : cornerInset;
                float left = rtl != hotkeyAtEnd ? getWidth() - inset - w : inset;
                float top = hotkeyAtEnd ? (getHeight() - h) / 2 : cornerInset - dp;
                box.set(left, top, left + w, top + h);
                paint.setStyle(Paint.Style.STROKE);
                paint.setStrokeWidth(dp);
                paint.setColor(hotkeyAtEnd || isChecked() ? getCurrentTextColor() : quiet);
                canvas.drawRoundRect(box, 3 * dp, 3 * dp, paint);
                paint.setStyle(Paint.Style.FILL);
                Paint.FontMetrics metrics = paint.getFontMetrics();
                canvas.drawText(hotkey, box.centerX() - text / 2, box.centerY() - (metrics.ascent + metrics.descent) / 2, paint);
                paint.setFakeBoldText(false);
            }
        }
    }

    /** Draws the controller caps on the tiles added from here on. */
    public void setCapsShown(boolean shown) {
        capsShown = shown;
    }

    /** A tile in this grid: the glass control fill, the selected tint while on, the ring on focus. */
    public Tile addTile(int icon, CharSequence label, CharSequence detail, Boolean on, String hotkey) {
        Context context = getContext();
        android.content.res.Resources res = context.getResources();
        Tile tile = new Tile(context);
        tile.setText(detail == null ? label : withDetail(context, label, detail));
        tile.setAllCaps(false);
        tile.setTextAppearance(context, R.style.TextAppearance_Nightjar_Tile);
        tile.setSingleLine(false);
        tile.setMaxLines(detail == null ? 2 : 3);
        tile.setEllipsize(android.text.TextUtils.TruncateAt.END);
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.M) {
            // Layout's constant (API 23) is LineBreaker's (API 29) by value; lint knows only the newer one.
            //noinspection WrongConstant
            tile.setBreakStrategy(android.text.Layout.BREAK_STRATEGY_BALANCED);
        }
        tile.setIconResource(icon);
        tile.setIconGravity(MaterialButton.ICON_GRAVITY_TOP);
        tile.setIconSize(res.getDimensionPixelSize(R.dimen.stream_tile_icon));
        tile.setIconPadding(res.getDimensionPixelSize(R.dimen.stream_tile_icon_gap));
        int minHeight = res.getDimensionPixelSize(R.dimen.stream_tile_min_height);
        tile.setMinHeight(minHeight);
        tile.setMinimumHeight(minHeight);
        tile.setMinWidth(0);
        tile.setMinimumWidth(0);
        tile.setInsetTop(0);
        tile.setInsetBottom(0);
        int side = res.getDimensionPixelSize(R.dimen.stream_tile_pad_side);
        tile.setPaddingRelative(side, res.getDimensionPixelSize(R.dimen.stream_tile_pad_top), side,
                res.getDimensionPixelSize(R.dimen.stream_tile_pad_bottom));
        int[][] states = {new int[] {android.R.attr.state_checked}, new int[] {}};
        android.content.res.ColorStateList ink = new android.content.res.ColorStateList(states, new int[] {
                MaterialColors.getColor(context, R.attr.glassOnSelected, 0),
                MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurface, 0)});
        tile.setTextColor(ink);
        tile.setIconTint(ink);
        tile.setBackground(new GlassControlDrawable(context, GlassControlDrawable.Kind.CONTROL, res.getDimension(R.dimen.stream_tile_corner)));
        tile.setCapShown(capsShown);
        tile.setOn(on);
        tile.setHotkey(hotkey);
        // On a TV the ring grows the tile a little, as the sheet's toggles do.
        if ((res.getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_TYPE_MASK)
                == android.content.res.Configuration.UI_MODE_TYPE_TELEVISION) {
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
        text.setSpan(new android.text.style.TextAppearanceSpan(context, R.style.TextAppearance_Nightjar_TileDetail),
                start, text.length(), 0);
        text.setSpan(new android.text.style.ForegroundColorSpan(MaterialColors.getColor(context,
                com.google.android.material.R.attr.colorOnSurfaceVariant, 0)), start, text.length(), 0);
        return text;
    }
}
