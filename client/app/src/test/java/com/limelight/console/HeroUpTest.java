package com.limelight.console;

import static org.junit.Assert.assertSame;

import android.app.Activity;
import android.view.View;
import android.widget.FrameLayout;

import androidx.annotation.NonNull;
import androidx.recyclerview.widget.LinearLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/** Up from the first row leaves the shelves: on a handheld's hero a free search only went between Play and the panel. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34)
public class HeroUpTest {
    @Test
    public void upFromTheFirstRowGoesWhereTheShelvesSay() {
        Activity activity = Robolectric.buildActivity(Activity.class).setup().get();
        FrameLayout root = new FrameLayout(activity);
        RecyclerView rows = new RecyclerView(activity);
        ShelfLayoutManager layout = new ShelfLayoutManager(activity, LinearLayoutManager.VERTICAL, () -> true);
        View tab = new View(activity);
        layout.setRowFocus(new ShelfLayoutManager.RowFocus() {
            @Override public View cardIn(RecyclerView list, View row, View from) {
                return null;
            }

            @Override public View aboveFirst(RecyclerView list, View row, View from) {
                return tab;
            }
        });
        rows.setLayoutManager(layout);
        rows.setAdapter(new RecyclerView.Adapter<RecyclerView.ViewHolder>() {
            @NonNull @Override public RecyclerView.ViewHolder onCreateViewHolder(@NonNull android.view.ViewGroup parent, int type) {
                View row = new FrameLayout(parent.getContext());
                row.setLayoutParams(new RecyclerView.LayoutParams(400, 100));
                row.setFocusable(true);
                return new RecyclerView.ViewHolder(row) { };
            }

            @Override public void onBindViewHolder(@NonNull RecyclerView.ViewHolder holder, int position) { }

            @Override public int getItemCount() {
                return 2;
            }
        });
        root.addView(rows, new FrameLayout.LayoutParams(400, 400));
        activity.setContentView(root);
        root.measure(View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, 400, 400);

        assertSame(tab, layout.onInterceptFocusSearch(rows.getChildAt(0), View.FOCUS_UP));
    }

    /** The top row's control straight above: the selected tab on the left, the PC capsule's side on the right. */
    @Test
    public void upGoesToWhatIsStraightAbove() {
        Activity activity = Robolectric.buildActivity(Activity.class).setup().get();
        android.widget.LinearLayout tabs = new android.widget.LinearLayout(activity);
        tabs.setId(com.limelight.R.id.shell_bar_items);
        View home = new View(activity);
        View settings = new View(activity);
        home.setSelected(true);
        tabs.addView(home, 100, 40);
        tabs.addView(settings, 100, 40);
        View capsule = new View(activity);
        capsule.setId(com.limelight.R.id.home_pc);
        FrameLayout root = new FrameLayout(activity);
        root.addView(tabs, new FrameLayout.LayoutParams(200, 40));
        root.addView(capsule, new FrameLayout.LayoutParams(100, 40, android.view.Gravity.END));
        View left = new View(activity);
        View right = new View(activity);
        root.addView(left, new FrameLayout.LayoutParams(100, 40, android.view.Gravity.BOTTOM));
        root.addView(right, new FrameLayout.LayoutParams(100, 40, android.view.Gravity.BOTTOM | android.view.Gravity.END));
        activity.setContentView(root);
        root.measure(View.MeasureSpec.makeMeasureSpec(1000, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, 1000, 400);

        assertSame(home, com.limelight.ui.ShellBar.nearestInTopRow(activity, left, com.limelight.R.id.home_pc));
        assertSame(capsule, com.limelight.ui.ShellBar.nearestInTopRow(activity, right, com.limelight.R.id.home_pc));
    }
}
