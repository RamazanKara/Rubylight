/**
 * The part of the Control Center that scrolls ({@link ControlCenterDock}): the selected module's cards and rows, or a
 * page one level down (the overlay, the PC's session, the shortcuts and their editor). Its rows are the ones a sheet has
 * ({@link ActionSheetRows}), so a page reads the same wherever it stands, and it remembers the row D-pad focus starts on.
 */
package com.limelight.ui;

import android.content.Context;
import android.view.ViewGroup;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.core.widget.NestedScrollView;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;

final class ControlCenterPage {
    private final Context context;
    private final NestedScrollView scroll;
    private final LinearLayout content;
    private final int spacing;
    private final boolean television;
    private boolean capsShown = true;
    private int windowHeightDp;
    private MaterialButton firstAction;

    ControlCenterPage(Context context) {
        this.context = context;
        television = ControlCenterCards.television(context);
        spacing = Math.round(8 * context.getResources().getDisplayMetrics().density);
        int edge = context.getResources().getDimensionPixelSize(R.dimen.stream_menu_edge);
        content = new LinearLayout(context);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(edge - spacing, 0, edge - spacing, spacing * 2);
        content.setClipToPadding(false);
        if (television) content.setClipChildren(false);
        scroll = new ActionSheet.StillScroll(context);
        scroll.setClipToPadding(false);
        // The rows go on under the footer: they fade there instead of ending on a cut edge.
        scroll.setVerticalFadingEdgeEnabled(true);
        scroll.setFadingEdgeLength(spacing * 3);
        scroll.addView(content);
    }

    NestedScrollView scroll() {
        return scroll;
    }

    LinearLayout content() {
        return content;
    }

    int spacing() {
        return spacing;
    }

    /** Draws the controller caps on the status rows added from here on, and says them; their buttons work either way. */
    void setCapsShown(boolean shown) {
        capsShown = shown;
    }

    int windowHeightDp() {
        return windowHeightDp > 0 ? windowHeightDp : context.getResources().getConfiguration().screenHeightDp;
    }

    void setWindowHeightDp(int windowHeightDp) {
        this.windowHeightDp = windowHeightDp;
        int heightDp = windowHeightDp();
        for (int i = 0; i < content.getChildCount(); i++) {
            android.view.View child = content.getChildAt(i);
            if (!(child instanceof TileGrid)) continue;
            TileGrid grid = (TileGrid) child;
            for (int j = 0; j < grid.getChildCount(); j++) {
                android.view.View item = grid.getChildAt(j);
                if (item instanceof TileGrid.Tile && ((TileGrid.Tile) item).hasTopStartHeader()) {
                    ControlCenterCards.fitCard((TileGrid.Tile) item, heightDp);
                }
            }
        }
    }

    /** Removes every card and row so another page can show, scrolled back to its top. */
    void clear() {
        content.removeAllViews();
        firstAction = null;
        scroll.scrollTo(0, 0);
    }

    /** The row D-pad focus starts on; null for a page with nothing to press. */
    MaterialButton firstAction() {
        return firstAction;
    }

    /** Makes another row the one D-pad focus starts on, such as one that does not end the stream. */
    void setFirstAction(MaterialButton button) {
        if (button == null || button == firstAction) return;
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
            if (firstAction != null) firstAction.setFocusedByDefault(false);
            button.setFocusedByDefault(true);
        }
        firstAction = button;
    }

    private void claim(MaterialButton button) {
        if (firstAction == null) setFirstAction(button);
    }

    /** A vertical box a page fills and refills by itself. */
    LinearLayout addGroup() {
        return ActionSheetRows.addGroup(content, context, television);
    }

    /** Text under a row, or on its own: quiet, or in the error colour. */
    TextView addNote(ViewGroup parent, CharSequence text, boolean error) {
        return ActionSheetRows.addNote(context, parent, text, error, spacing);
    }

    /** A grid of compact tiles, four to a row: the shortcuts page's. */
    TileGrid addTileGrid() {
        return ActionSheetRows.addTileGrid(content, context, spacing, television);
    }

    /** A module's cards, two to a row. */
    TileGrid addCardGrid() {
        TileGrid grid = ControlCenterCards.grid(context);
        content.addView(grid, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        return grid;
    }

    TextView addSection(int title) {
        TextView heading = ActionSheetRows.sectionHeading(context, context.getText(title), spacing, !television);
        content.addView(heading);
        return heading;
    }

    MaterialButton addAction(int icon, CharSequence label) {
        return addAction(icon, label, com.google.android.material.R.attr.colorOnSurface,
                com.google.android.material.R.attr.colorOnSurfaceVariant);
    }

    /** An action with a second, quieter line that says what will happen. */
    MaterialButton addAction(int icon, CharSequence label, CharSequence note) {
        return addAction(icon, ActionSheetRows.withNote(context, label, note));
    }

    /** An action that removes or ends something, in the error colour. */
    MaterialButton addDestructiveAction(int icon, CharSequence label) {
        return addAction(icon, label, androidx.appcompat.R.attr.colorError, androidx.appcompat.R.attr.colorError);
    }

    /** An action that opens a page one level down, marked with a chevron at its end. */
    MaterialButton addPageAction(int icon, CharSequence label) {
        return ActionSheetRows.wrapPageAction(content, context, addAction(icon, label), spacing, television);
    }

    private MaterialButton addAction(int icon, CharSequence label, int textColor, int iconColor) {
        MaterialButton button = ActionSheetRows.addAction(context, content, icon, label, textColor, iconColor, spacing, television);
        claim(button);
        return button;
    }

    void addDivider() {
        ActionSheetRows.addDivider(content, context, spacing);
    }

    /** A row of equal toggle buttons, for settings changed without leaving the page. */
    LinearLayout addToggleRow() {
        LinearLayout row = new ActionSheet.ToggleRow(context, spacing, spacing * 9);
        row.setPadding(spacing, spacing, spacing, spacing);
        if (television) {
            row.setClipToPadding(false);
            row.setClipChildren(false);
        }
        content.addView(row, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        return row;
    }

    /** A toggle in {@code row}, filled while on. A click flips {@code isChecked()} before its listener runs. */
    MaterialButton addToggle(LinearLayout row, int icon, CharSequence label, boolean checked) {
        MaterialButton button = ActionSheetRows.addToggle(context, row, icon, label, checked, spacing, false, spacing * 9, television);
        claim(button);
        return button;
    }

    /** A fact with its label at the start and its value at the end. */
    void addInfo(CharSequence label, CharSequence value) {
        ActionSheetRows.addInfo(context, content, label, value, spacing);
    }

    /** Rows standing together on one flat tint, hairlines between them. */
    LinearLayout addRowGroup() {
        return ActionSheetRows.addRowGroup(content, context, spacing);
    }

    /** A row in {@link #addRowGroup}: its name, a value at its end, the controller button that opens it, a chevron. */
    MenuRow addStatusRow(LinearLayout group, CharSequence label, CharSequence value, String hotkey) {
        MenuRow row = new MenuRow(context, label, value);
        row.setHotkey(hotkey, capsShown);
        // A finger's reach, whatever the row's own text needs.
        row.setMinimumHeight(Math.max(row.getMinimumHeight(),
                context.getResources().getDimensionPixelSize(R.dimen.nj_size_touch_min)));
        group.addView(row, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT));
        claim(row);
        return row;
    }

    /** The wide card of a slider, its track inside; answers the line showing the value. */
    TextView addSliderCard(CharSequence label, CharSequence value, StepSlider slider) {
        return ControlCenterCards.sliderCard(context, content, label, value, slider);
    }

    /** A quiet line with nothing to press: a notice of the stream's, or something the PC reports. */
    TextView addNotice(int icon, CharSequence text) {
        return ControlCenterCards.notice(context, content, icon, text);
    }
}
