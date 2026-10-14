package com.limelight.demo;

import android.content.Context;
import android.content.Intent;
import android.database.DataSetObserver;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.AbsListView;
import android.widget.BaseAdapter;
import android.widget.FrameLayout;
import android.widget.ListAdapter;
import android.widget.TextView;

import com.limelight.AppView;
import com.limelight.R;

public final class DemoAppView extends AppView {
    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override public void receiveAbsListView(AbsListView list) {
        super.receiveAbsListView(list);
        // After a rotation the retained fragment can call this before the adapter exists; the real one is set later.
        if (list.getAdapter() != null) list.setAdapter(new TidyCards(list.getAdapter()));
        list.setOnItemClickListener((parent, view, position, id) -> startActivity(
                new Intent(this, DemoGameActivity.class).putExtra("state", "statistics")));
    }

    /**
     * Presentation-only adjustments to the real library cards for the marketing captures: cards that are not running
     * give up the invisible "Resume / quit" slot, so the grid is not full of empty space under the titles (the two-line
     * title slot stays, which keeps every card in a row the same height), and the "Running" chip moves to the top
     * corner, clear of the wordmark printed on the cover art.
     */
    private static final class TidyCards extends BaseAdapter {
        private final ListAdapter delegate;

        TidyCards(ListAdapter delegate) {
            this.delegate = delegate;
        }

        @Override public View getView(int position, View convertView, ViewGroup parent) {
            View card = delegate.getView(position, convertView, parent);
            View actions = card.findViewById(R.id.grid_actions);
            if (actions.getVisibility() != View.VISIBLE) actions.setVisibility(View.GONE);
            TextView status = card.findViewById(R.id.grid_status);
            status.setTextSize(TypedValue.COMPLEX_UNIT_SP, 13);
            float dp = card.getResources().getDisplayMetrics().density;
            status.setPadding(Math.round(8 * dp), Math.round(4 * dp), Math.round(8 * dp), Math.round(4 * dp));
            ((FrameLayout.LayoutParams) status.getLayoutParams()).gravity = Gravity.TOP | Gravity.END;
            return card;
        }

        @Override public int getCount() { return delegate.getCount(); }
        @Override public Object getItem(int position) { return delegate.getItem(position); }
        @Override public long getItemId(int position) { return delegate.getItemId(position); }
        @Override public boolean hasStableIds() { return delegate.hasStableIds(); }
        @Override public int getViewTypeCount() { return delegate.getViewTypeCount(); }
        @Override public int getItemViewType(int position) { return delegate.getItemViewType(position); }
        @Override public boolean areAllItemsEnabled() { return delegate.areAllItemsEnabled(); }
        @Override public boolean isEnabled(int position) { return delegate.isEnabled(position); }
        @Override public void registerDataSetObserver(DataSetObserver observer) { delegate.registerDataSetObserver(observer); }
        @Override public void unregisterDataSetObserver(DataSetObserver observer) { delegate.unregisterDataSetObserver(observer); }
    }
}
