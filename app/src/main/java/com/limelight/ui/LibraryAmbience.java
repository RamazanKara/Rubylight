package com.limelight.ui;

import android.view.Choreographer;
import android.view.View;
import android.widget.AbsListView;
import android.widget.AdapterView;
import android.widget.TextView;

import com.limelight.AppView;
import com.limelight.grid.AppGridAdapter;

/**
 * What the TV library shows about the game that has the selection: its title and chips on the focus line, and its
 * cover as the screen's backdrop. The selection moves in {@code onKeyDown}; this follows it a moment later, and
 * only once per frame for the text and once per settle for the backdrop, so a held key costs neither.
 */
public final class LibraryAmbience implements Choreographer.FrameCallback {
    private final BackdropView backdrop;
    private final TextView title;
    private final TextView status;
    private AppGridAdapter adapter;
    private AbsListView grid;

    private AppView.AppObject pending;
    private boolean queued;
    private boolean cleared;

    /** Either part may be missing (a layout without the backdrop, as on a phone). */
    public LibraryAmbience(BackdropView backdrop, TextView title, TextView status) {
        this.backdrop = backdrop;
        this.title = title;
        this.status = status;
    }

    /** Follows the grid's selection. The adapter also says when a cover that was still loading has arrived. */
    public void bind(AbsListView grid, AppGridAdapter adapter) {
        this.grid = grid;
        this.adapter = adapter;
        grid.setOnItemSelectedListener(new AdapterView.OnItemSelectedListener() {
            @Override public void onItemSelected(AdapterView<?> parent, View view, int position, long id) {
                Object item = position >= 0 && position < parent.getCount() ? parent.getItemAtPosition(position) : null;
                if (item instanceof AppView.AppObject) select((AppView.AppObject) item);
            }

            @Override public void onNothingSelected(AdapterView<?> parent) { }
        });
        adapter.setArtListener(app -> {
            AppView.AppObject selected = selected();
            if (selected != null && selected.app.getAppId() == app.getAppId()) askBackdrop(selected, false);
        });
    }

    private AppView.AppObject selected() {
        if (grid == null) return null;
        Object item = grid.getSelectedItem();
        return item instanceof AppView.AppObject ? (AppView.AppObject) item : null;
    }

    /** The selected game's chips can change under it (it starts or stops running, is hidden); say so on the line. */
    public void refresh() {
        AppView.AppObject app = selected();
        if (app == null) return;
        pending = app;
        cleared = false;
        queueLine();
    }

    private void queueLine() {
        if (!queued && (title != null || status != null)) {
            queued = true;
            Choreographer.getInstance().postFrameCallback(this);
        }
    }

    private void select(AppView.AppObject app) {
        pending = app;
        cleared = false;
        queueLine();
        askBackdrop(app, grid instanceof AdaptiveGridView && ((AdaptiveGridView) grid).isFastTravel());
    }

    private void askBackdrop(AppView.AppObject app, boolean fast) {
        if (backdrop != null && adapter != null) backdrop.focus(String.valueOf(app.app.getAppId()), adapter.coverOf(app), fast);
    }

    /** The grid has no games to select (a search with no match, a host with none). */
    public void clear() {
        pending = null;
        cleared = true;
        if (title != null) title.setText("");
        if (status != null) status.setVisibility(View.GONE);
    }

    @Override
    public void doFrame(long frameTimeNanos) {
        queued = false;
        AppView.AppObject app = pending;
        if (app == null || cleared) return;
        // The same game told twice (the grid repeats its selection after a layout) costs no text layout.
        if (title != null && !app.app.getAppName().contentEquals(title.getText())) title.setText(app.app.getAppName());
        if (status != null) AppGridAdapter.bindStatusChip(status, app);
    }
}
