/**
 * What the Control Center is made of ({@link ControlCenterDock}): a module's entry on the rail, a card (icon, short name,
 * state; amber while its switch is on), the wide card that holds a slider's track, the quiet notice and status rows, and
 * the primary way back to the stream. Every colour is one of the {@code surface.controlCenter.*} tokens, which are night
 * in every theme: nothing here asks the theme for a surface colour, so a day theme cannot put a white card over a game.
 */
package com.limelight.ui;

import android.content.Context;
import android.content.res.ColorStateList;
import android.content.res.Resources;
import android.graphics.Canvas;
import android.graphics.ColorFilter;
import android.graphics.Outline;
import android.graphics.Paint;
import android.graphics.PixelFormat;
import android.graphics.RectF;
import android.graphics.drawable.Drawable;
import android.view.Gravity;
import android.view.View;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.core.content.ContextCompat;
import androidx.core.view.ViewCompat;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.limelight.R;

final class ControlCenterCards {
    private ControlCenterCards() { }

    /** A module's icon on the rail. */
    static int icon(String module) {
        switch (module) {
            case ControlCenterModel.PICTURE: return R.drawable.ic_monitor;
            case ControlCenterModel.AUDIO: return R.drawable.ic_speaker;
            case ControlCenterModel.CLIPS: return R.drawable.ic_video;
            case ControlCenterModel.PC: return R.drawable.ic_computer;
            case ControlCenterModel.CONTROLLER: return R.drawable.ic_gamepad;
            case ControlCenterModel.GUESTS: return R.drawable.ic_guests;
            case ControlCenterModel.POWER: return R.drawable.ic_power;
            default: return R.drawable.ic_glass_grid;
        }
    }

    /** A module's name: the {@code control_center_<key>} string, or 0 for a module this client does not know. */
    static int label(String module) {
        switch (module) {
            case ControlCenterModel.QUICK: return R.string.control_center_quick;
            case ControlCenterModel.PICTURE: return R.string.control_center_picture;
            case ControlCenterModel.AUDIO: return R.string.control_center_audio;
            case ControlCenterModel.CLIPS: return R.string.control_center_clips;
            case ControlCenterModel.PC: return R.string.control_center_pc;
            case ControlCenterModel.CONTROLLER: return R.string.control_center_controller;
            case ControlCenterModel.GUESTS: return R.string.control_center_guests;
            case ControlCenterModel.POWER: return R.string.control_center_power;
            default: return 0;
        }
    }

    // Under this height, in dp, a window gets the low cards: upright phones and tablets are over it, a phone on its side
    // and a handheld under it.
    private static final int LOW_WINDOW_DP = 480;

    static boolean television(Context context) {
        return (context.getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_TYPE_MASK)
                == android.content.res.Configuration.UI_MODE_TYPE_TELEVISION;
    }

    /**
     * The tint under a card, a rail entry or the primary way back: flat over the panel, never a blur of its own. On (a
     * checked card, the selected module) it takes the amber container and rim; hovered or pressed it lifts; focused, the
     * ring stands on its own edge, inside its bounds, so the panel's clip never cuts it and nothing moves.
     */
    static final class Tint extends Drawable {
        enum Kind { CARD, RAIL, PRIMARY }

        private final Kind kind;
        private final float radius, rimWidth, ringWidth;
        private final int fillRest, fillOn, rimRest, rimOn, lift, ringColor;
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final RectF rect = new RectF();
        private boolean on, focused, active;

        Tint(Context context, Kind kind, float radius) {
            this.kind = kind;
            this.radius = radius;
            Resources res = context.getResources();
            boolean highContrast = AppTheme.isHighContrast(context);
            float density = res.getDisplayMetrics().density;
            rimWidth = highContrast ? 2 * density : Math.max(1f, density);
            ringWidth = res.getDimension(highContrast ? R.dimen.nj_hc_focus_ring_width
                    : television(context) ? R.dimen.nj_focus_ring_width_tv : R.dimen.nj_focus_ring_width);
            int moon = ContextCompat.getColor(context, highContrast ? R.color.nj_hc_color_moon : R.color.nj_color_moon);
            lift = ContextCompat.getColor(context, R.color.nj_focus_fill_lift);
            if (kind == Kind.PRIMARY) {
                fillRest = fillOn = moon;
                rimRest = rimOn = 0;
                // The amber ring would vanish on the amber capsule.
                ringColor = MaterialColors.getColor(StreamGlass.themed(context), R.attr.glassRingOnMoon, 0xFFFFFFFF);
            } else if (highContrast) {
                fillRest = 0;
                fillOn = (moon & 0x00FFFFFF) | 0x40000000;
                rimRest = kind == Kind.RAIL ? 0 : ContextCompat.getColor(context, R.color.nj_hc_color_rim);
                rimOn = moon;
                ringColor = ContextCompat.getColor(context, R.color.nj_focus_ring_color);
            } else {
                fillRest = kind == Kind.RAIL ? 0 : ContextCompat.getColor(context, R.color.nj_surface_control_center_card_fill);
                fillOn = ContextCompat.getColor(context, R.color.nj_surface_control_center_card_fill_on);
                rimRest = kind == Kind.RAIL ? 0 : ContextCompat.getColor(context, R.color.nj_surface_control_center_card_rim);
                rimOn = ContextCompat.getColor(context, R.color.nj_surface_control_center_card_rim_on);
                ringColor = ContextCompat.getColor(context, R.color.nj_focus_ring_color);
            }
        }

        /** The fill and rim it draws now: for a test. */
        int fill() {
            return on ? fillOn : fillRest;
        }

        int rim() {
            return on ? rimOn : rimRest;
        }

        float rimWidth() {
            return rimWidth;
        }

        boolean ringShown() {
            return focused;
        }

        @Override
        public boolean isStateful() {
            return true;
        }

        @Override
        protected boolean onStateChange(int[] states) {
            boolean nowOn = false, nowFocused = false, nowActive = false;
            for (int state : states) {
                if (state == android.R.attr.state_checked || state == android.R.attr.state_selected) nowOn = true;
                else if (state == android.R.attr.state_focused) nowFocused = true;
                else if (state == android.R.attr.state_pressed || state == android.R.attr.state_hovered) nowActive = true;
            }
            if (nowOn == on && nowFocused == focused && nowActive == active) return false;
            on = nowOn;
            focused = nowFocused;
            active = nowActive;
            invalidateSelf();
            return true;
        }

        private float corner(float inset) {
            float half = Math.min(getBounds().width(), getBounds().height()) / 2f;
            return Math.max(0f, Math.min(radius, half) - inset);
        }

        @Override
        public void draw(Canvas canvas) {
            rect.set(getBounds());
            paint.setStyle(Paint.Style.FILL);
            if (fill() != 0) {
                paint.setColor(fill());
                canvas.drawRoundRect(rect, corner(0), corner(0), paint);
            }
            if (active) {
                paint.setColor(lift);
                canvas.drawRoundRect(rect, corner(0), corner(0), paint);
            }
            paint.setStyle(Paint.Style.STROKE);
            if (rim() != 0) {
                float inset = rimWidth / 2f;
                rect.set(getBounds());
                rect.inset(inset, inset);
                paint.setStrokeWidth(rimWidth);
                paint.setColor(rim());
                canvas.drawRoundRect(rect, corner(inset), corner(inset), paint);
            }
            if (focused) {
                float inset = ringWidth / 2f;
                rect.set(getBounds());
                rect.inset(inset, inset);
                paint.setStrokeWidth(ringWidth);
                paint.setColor(ringColor);
                canvas.drawRoundRect(rect, corner(inset), corner(inset), paint);
            }
        }

        @Override
        public void getOutline(Outline outline) {
            outline.setRoundRect(getBounds(), corner(0));
        }

        @Override
        public void setAlpha(int alpha) {
            paint.setAlpha(alpha);
        }

        @Override
        public void setColorFilter(ColorFilter filter) {
            // Nothing: a MaterialButton hands a background that is not its own the tint of its style as a colour filter,
            // clear at rest, which would paint every card and Return to stream away. These colours are its own.
        }

        @Override
        public int getOpacity() {
            return PixelFormat.TRANSLUCENT;
        }
    }

    /** The label ink: the label's own colour at rest, amber for the icon of a switch that is on. */
    private static ColorStateList ink(Context context, boolean amberWhenOn) {
        int label = ContextCompat.getColor(context, R.color.nj_surface_control_center_label);
        int moon = ContextCompat.getColor(context,
                AppTheme.isHighContrast(context) ? R.color.nj_hc_color_moon : R.color.nj_color_moon);
        return new ColorStateList(new int[][] {new int[] {android.R.attr.state_checked},
                new int[] {android.R.attr.state_selected}, new int[] {}},
                new int[] {amberWhenOn ? moon : label, amberWhenOn ? moon : label, label});
    }

    /** The cards of a page, two to a row. */
    static TileGrid grid(Context context) {
        Resources res = context.getResources();
        int gap = res.getDimensionPixelSize(R.dimen.control_center_gap);
        TileGrid grid = new TileGrid(context, gap, 2, 96);
        int pad = res.getDimensionPixelSize(R.dimen.control_center_gap);
        grid.setPadding(pad, pad / 2, pad, pad / 2);
        if (television(context)) {
            grid.setClipToPadding(false);
            grid.setClipChildren(false);
        }
        return grid;
    }

    /**
     * A card: its icon at top-start, optional controller cap at top-end, and full-width short name + state below,
     * matching desktop Nightjar's Control Center cards so single words never split mid-word on compact docks.
     */
    static TileGrid.Tile card(Context context, int icon, CharSequence label, CharSequence state, Boolean on, String hotkey,
                              boolean capsShown) {
        return card(context, icon, label, state, on, hotkey, capsShown, context.getResources().getConfiguration().screenHeightDp);
    }

    static TileGrid.Tile card(Context context, int icon, CharSequence label, CharSequence state, Boolean on, String hotkey,
                              boolean capsShown, int windowHeightDp) {
        Resources res = context.getResources();
        float density = res.getDisplayMetrics().density;
        boolean tv = television(context);
        TileGrid.Tile card = new TileGrid.Tile(context);
        card.setAllCaps(false);
        card.setTextAppearance(context, tv ? R.style.TextAppearance_Nightjar_Label : R.style.TextAppearance_Nightjar_Tile);
        if (!tv) {
            card.setTextSize(android.util.TypedValue.COMPLEX_UNIT_SP, 12.5f);
            androidx.core.widget.TextViewCompat.setLineHeight(card, Math.round(16 * res.getDisplayMetrics().scaledDensity));
        }
        card.setTextColor(ContextCompat.getColor(context, R.color.nj_surface_control_center_label));
        card.setSingleLine(false);
        card.setMaxLines(3);
        card.setEllipsize(android.text.TextUtils.TruncateAt.END);
        //noinspection WrongConstant
        card.setBreakStrategy(android.text.Layout.BREAK_STRATEGY_SIMPLE);
        card.setHyphenationFrequency(android.text.Layout.HYPHENATION_FREQUENCY_NONE);
        card.setGravity(Gravity.START | Gravity.BOTTOM);
        card.setTextAlignment(View.TEXT_ALIGNMENT_VIEW_START);
        int height = cardHeight(context, windowHeightDp);
        card.setMinHeight(height);
        card.setMinimumHeight(height);
        card.setMinWidth(0);
        card.setMinimumWidth(0);
        card.setInsetTop(0);
        card.setInsetBottom(0);
        int side = res.getDimensionPixelSize(R.dimen.control_center_card_pad);
        card.setHotkey(hotkey);
        card.setHotkeyAtEnd(true, side);
        card.setCapShown(capsShown);
        card.setTopStartHeader(icon, res.getDimensionPixelSize(R.dimen.control_center_card_icon),
                ink(context, true), side, Math.round(9 * density), Math.round(4 * density), Math.round(8 * density));
        fitCard(card, windowHeightDp);
        card.setBackground(new Tint(context, Tint.Kind.CARD, res.getDimension(R.dimen.nj_radius_control)));
        card.setOn(on);
        if (tv) card.setStateListAnimator(Motion.controlFocus(context));
        write(card, label, state);
        return card;
    }

    static void fitCard(TileGrid.Tile card, int windowHeightDp) {
        Context context = card.getContext();
        Resources res = context.getResources();
        float density = res.getDisplayMetrics().density;
        int height = cardHeight(context, windowHeightDp);
        int side = res.getDimensionPixelSize(R.dimen.control_center_card_pad);
        boolean low = isLowWindow(context, windowHeightDp);
        int iconSize = low ? Math.round(16 * density) : res.getDimensionPixelSize(R.dimen.control_center_card_icon);
        int topPad = Math.round((low ? 5 : 9) * density);
        int iconGap = Math.round((low ? 2 : 4) * density);
        int bottomPad = Math.round((low ? 5 : 8) * density);
        card.fitHeader(height, iconSize, side, topPad, iconGap, bottomPad);
    }

    static boolean isLowWindow(Context context, int windowHeightDp) {
        return !television(context) && windowHeightDp > 0 && windowHeightDp < LOW_WINDOW_DP;
    }

    /**
     * A card's least height: the design's, or the low one where the window is too low to show two rows of those between
     * the header and the ways out, as a handheld's and a phone's on its side are. A TV has the height.
     */
    static int cardHeight(Context context) {
        return cardHeight(context, context.getResources().getConfiguration().screenHeightDp);
    }

    static int cardHeight(Context context, int windowHeightDp) {
        Resources res = context.getResources();
        return res.getDimensionPixelSize(isLowWindow(context, windowHeightDp)
                ? R.dimen.control_center_card_low : R.dimen.nj_size_control_center_card);
    }

    /** A card's name and, under it, its state; also what a screen reader says for it. */
    static void write(TileGrid.Tile card, CharSequence label, CharSequence state) {
        Context context = card.getContext();
        if (state == null || state.length() == 0) {
            card.setText(label);
        } else {
            android.text.SpannableStringBuilder text = new android.text.SpannableStringBuilder(label).append('\n');
            int start = text.length();
            text.append(state);
            text.setSpan(new android.text.style.TextAppearanceSpan(context, R.style.TextAppearance_Nightjar_TileDetail),
                    start, text.length(), 0);
            if (!television(context)) {
                text.setSpan(new android.text.style.AbsoluteSizeSpan(11, true), start, text.length(), 0);
            }
            text.setSpan(new android.text.style.ForegroundColorSpan(ContextCompat.getColor(context,
                    R.color.nj_surface_control_center_label_secondary)), start, text.length(), 0);
            card.setText(text);
        }
        StringBuilder said = new StringBuilder(label);
        // A switch says its own state; anything else says the line under its name.
        if (!card.isCheckable() && state != null && state.length() > 0) said.append(", ").append(state);
        if (card.hotkey() != null && card.capShown()) said.append(", ").append(card.hotkey());
        card.setContentDescription(said);
    }

    /** The state line of a switch: On or Off, in the words the app already has. */
    static CharSequence switchState(Context context, boolean on) {
        return context.getString(on ? R.string.native_on : R.string.native_off);
    }

    /**
     * The wide card of a slider: its name and value over the track, which lies inside the card. The card takes the ring
     * while the slider has the focus; D-pad left and right move the slider. Answers the line showing the value.
     */
    static TextView sliderCard(Context context, LinearLayout content, CharSequence label, CharSequence value, StepSlider slider) {
        Resources res = context.getResources();
        int gap = res.getDimensionPixelSize(R.dimen.control_center_gap);
        LinearLayout card = new LinearLayout(context);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setGravity(Gravity.CENTER_VERTICAL);
        card.setMinimumHeight(res.getDimensionPixelSize(R.dimen.nj_size_control_center_card));
        card.setBackground(new Tint(context, Tint.Kind.CARD, res.getDimension(R.dimen.nj_radius_control)));
        TextView shown = ActionSheetRows.addSliderRow(context, card, label, value, slider, gap);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        params.setMargins(gap, gap / 2, gap, gap / 2);
        content.addView(card, params);
        return shown;
    }

    /**
     * A module's entry on the rail: its icon over its name, amber while it is the one shown. {@code vertical} for the dock's
     * rail down the side, else the sheet's rail across its top.
     */
    static MaterialButton railEntry(Context context, String module, boolean vertical) {
        Resources res = context.getResources();
        int name = label(module);
        CharSequence text = name == 0 ? module : context.getString(name);
        MaterialButton entry = new MaterialButton(context, null, androidx.appcompat.R.attr.borderlessButtonStyle);
        entry.setText(text);
        entry.setAllCaps(false);
        entry.setTextAppearance(context, R.style.TextAppearance_Nightjar_Tile);
        entry.setTextColor(ink(context, true));
        // One line by its line count, not singleLine: a single line scrolls sideways, so it never counts as too long.
        entry.setMaxLines(1);
        entry.setEllipsize(android.text.TextUtils.TruncateAt.END);
        // A long name in another language gets smaller before it is cut; not on a TV, where nothing is under its caption size.
        int size = Math.max(10, Math.round(entry.getTextSize() / res.getDisplayMetrics().scaledDensity));
        if (!television(context)) {
            androidx.core.widget.TextViewCompat.setAutoSizeTextTypeUniformWithConfiguration(entry, Math.min(9, size - 1), size, 1,
                    android.util.TypedValue.COMPLEX_UNIT_SP);
        }
        entry.setIconResource(icon(module));
        entry.setIconGravity(MaterialButton.ICON_GRAVITY_TOP);
        entry.setIconSize(res.getDimensionPixelSize(R.dimen.control_center_card_icon));
        entry.setIconPadding(res.getDimensionPixelSize(R.dimen.control_center_rail_icon_gap));
        entry.setIconTint(ink(context, true));
        int touch = res.getDimensionPixelSize(R.dimen.control_center_rail_entry);
        entry.setMinHeight(touch);
        entry.setMinimumHeight(touch);
        entry.setMinWidth(0);
        entry.setMinimumWidth(0);
        entry.setInsetTop(0);
        entry.setInsetBottom(0);
        int pad = res.getDimensionPixelSize(R.dimen.control_center_rail_pad);
        entry.setPadding(pad, pad * 2, pad, pad);
        entry.setBackground(new Tint(context, Tint.Kind.RAIL, res.getDimension(R.dimen.nj_radius_tile)));
        entry.setContentDescription(text);
        entry.setTag(module);
        return entry;
    }

    /**
     * A rail entry at {@code height} on the dock's rail: {@code tight} where that is less than the design's, and the room
     * over its icon goes too. Changes nothing, and asks for no layout, when it already is so.
     */
    static void fitRailEntry(MaterialButton entry, int height, boolean tight) {
        int pad = entry.getResources().getDimensionPixelSize(R.dimen.control_center_rail_pad);
        int top = tight ? pad : pad * 2;
        if (entry.getMinimumHeight() == height && entry.getPaddingTop() == top) return;
        entry.setMinHeight(height);
        entry.setMinimumHeight(height);
        entry.setPadding(pad, top, pad, pad);
    }

    /** The cap of a trigger at an end of the rail: the triggers step through the modules. Never focused, never said. */
    static TextView triggerHint(Context context, String cap) {
        Resources res = context.getResources();
        float density = res.getDisplayMetrics().density;
        TextView hint = new TextView(context);
        hint.setText(cap);
        hint.setTextSize(android.util.TypedValue.COMPLEX_UNIT_PX, res.getDimension(R.dimen.stream_tile_cap_text));
        hint.setTypeface(hint.getTypeface(), android.graphics.Typeface.BOLD);
        hint.setTextColor(ContextCompat.getColor(context, R.color.nj_surface_control_center_label_secondary));
        hint.setGravity(Gravity.CENTER);
        android.graphics.drawable.GradientDrawable box = new android.graphics.drawable.GradientDrawable();
        box.setCornerRadius(3 * density);
        box.setStroke(Math.max(1, Math.round(density)),
                ContextCompat.getColor(context, R.color.nj_surface_control_center_label_secondary));
        hint.setBackground(box);
        hint.setPadding(Math.round(5 * density), Math.round(density), Math.round(5 * density), Math.round(density));
        hint.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
        hint.setFocusable(false);
        return hint;
    }

    /** What a pad calls its triggers: left first. A PlayStation pad says L2 and R2, a Nintendo one ZL and ZR. */
    static String[] triggerCaps(int padVendor) {
        if (padVendor == InputSource.SONY) return new String[] {"L2", "R2"};
        if (padVendor == InputSource.NINTENDO) return new String[] {"ZL", "ZR"};
        return new String[] {"LT", "RT"};
    }

    /** A quiet line on a page: something the stream says, with nothing to press. A screen reader reads it in its place. */
    static TextView notice(Context context, LinearLayout content, int icon, CharSequence text) {
        Resources res = context.getResources();
        int gap = res.getDimensionPixelSize(R.dimen.control_center_gap);
        TextView notice = new TextView(context);
        notice.setText(text);
        notice.setTextAppearance(context, television(context)
                ? R.style.TextAppearance_Nightjar_Supporting : R.style.TextAppearance_Nightjar_Caption);
        notice.setTextColor(ContextCompat.getColor(context, R.color.nj_surface_control_center_label_secondary));
        notice.setGravity(Gravity.CENTER_VERTICAL);
        Drawable mark = androidx.appcompat.content.res.AppCompatResources.getDrawable(context, icon);
        if (mark != null) {
            int size = res.getDimensionPixelSize(R.dimen.stream_menu_footer_icon);
            mark = mark.mutate();
            mark.setBounds(0, 0, size, size);
            mark.setTint(ContextCompat.getColor(context, R.color.nj_surface_control_center_label_secondary));
            notice.setCompoundDrawablesRelative(mark, null, null, null);
            notice.setCompoundDrawablePadding(gap);
        }
        android.graphics.drawable.GradientDrawable fill = new android.graphics.drawable.GradientDrawable();
        fill.setCornerRadius(res.getDimension(R.dimen.nj_radius_tile));
        fill.setColor(AppTheme.isHighContrast(context) ? 0 : ContextCompat.getColor(context, R.color.nj_surface_control_center_rail_fill));
        if (AppTheme.isHighContrast(context)) {
            fill.setStroke(Math.round(2 * res.getDisplayMetrics().density), ContextCompat.getColor(context, R.color.nj_hc_color_rim));
        }
        notice.setBackground(fill);
        notice.setPadding(gap * 3 / 2, gap, gap * 3 / 2, gap);
        notice.setMinHeight(res.getDimensionPixelSize(R.dimen.stream_menu_row_height));
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        params.setMargins(gap, gap / 2, gap, gap / 2);
        content.addView(notice, params);
        return notice;
    }

    /**
     * Return to stream, the Control Center's one amber control: a capsule as wide as the footer. {@code back} is the key
     * that also does it ({@code "B"}), said with its name while a controller is there.
     */
    static MaterialButton primary(Context context, CharSequence label, String back) {
        Resources res = context.getResources();
        MaterialButton button = new MaterialButton(context, null, androidx.appcompat.R.attr.borderlessButtonStyle);
        int ink = ContextCompat.getColor(context, AppTheme.isHighContrast(context) ? android.R.color.black : R.color.nj_color_on_moon);
        button.setText(label);
        button.setAllCaps(false);
        button.setTextAppearance(context, R.style.TextAppearance_Nightjar_Label);
        if (!television(context)) {
            button.setTextSize(android.util.TypedValue.COMPLEX_UNIT_SP, 13.5f);
        }
        button.setTextColor(ink);
        button.setSingleLine(true);
        button.setEllipsize(android.text.TextUtils.TruncateAt.END);
        button.setIconResource(R.drawable.ic_glass_play);
        button.setIconTint(ColorStateList.valueOf(ink));
        button.setIconSize(res.getDimensionPixelSize(R.dimen.stream_menu_footer_icon));
        button.setIconPadding(res.getDimensionPixelSize(R.dimen.control_center_gap));
        button.setIconGravity(MaterialButton.ICON_GRAVITY_TEXT_START);
        int height = res.getDimensionPixelSize(R.dimen.nj_size_touch_min);
        button.setMinHeight(height);
        button.setMinimumHeight(height);
        button.setInsetTop(0);
        button.setInsetBottom(0);
        button.setBackground(new Tint(context, Tint.Kind.PRIMARY, height));
        button.setContentDescription(back == null ? label : label + ", " + back);
        ViewCompat.setAccessibilityHeading(button, false);
        if (television(context)) button.setStateListAnimator(Motion.rowFocus(context));
        return button;
    }
}
