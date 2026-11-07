package com.limelight.preferences;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.preference.Preference;
import android.preference.PreferenceCategory;
import android.util.TypedValue;
import android.view.View;
import android.widget.ListView;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.ui.NightSkyView;

/**
 * A preference list whose rows stand in groups: one flat tint with a rim behind each run of rows, a hairline between rows. A
 * flat tint on the sky, never glass (no nested blur). Headings and spacers end a group. One draw per group, no layer.
 */
public final class GroupedListView extends ListView {
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint rim = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint hairline = new Paint();
    private final RectF box = new RectF();
    private final float radius;
    private final float inset;

    GroupedListView(Context context) {
        super(context);
        setId(android.R.id.list);
        radius = getResources().getDimension(R.dimen.nj_radius_group);
        inset = getResources().getDimension(R.dimen.settings_row_inset);
        fill.setColor(NightSkyView.groupFill(context));
        rim.setStyle(Paint.Style.STROKE);
        rim.setStrokeWidth(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, 1, getResources().getDisplayMetrics()));
        rim.setColor(MaterialColors.getColor(this, R.attr.glassGroupRim));
        hairline.setColor(NightSkyView.separator(context));
    }

    /** The group draws its own hairlines; PreferenceFragment sets the theme's full-width divider after onCreateView. */
    @Override
    public void setDivider(android.graphics.drawable.Drawable divider) {
        super.setDivider(null);
    }

    /** Puts a grouped list where a preference fragment's list is, before the fragment binds it. */
    public static ListView adopt(View root) {
        ListView old = root.findViewById(android.R.id.list);
        if (old == null || old instanceof GroupedListView) return old;
        android.view.ViewGroup parent = (android.view.ViewGroup) old.getParent();
        int index = parent.indexOfChild(old);
        GroupedListView list = new GroupedListView(old.getContext());
        list.setVerticalScrollBarEnabled(false);
        list.setOverScrollMode(OVER_SCROLL_NEVER);
        parent.removeViewAt(index);
        parent.addView(list, index, old.getLayoutParams());
        return list;
    }

    /** Whether the row at this position stands in a group. */
    private boolean grouped(int position) {
        if (position < 0 || position >= getCount()) return false;
        Object item = getItemAtPosition(position);
        return item instanceof Preference && !(item instanceof PreferenceCategory);
    }

    @Override
    protected void dispatchDraw(Canvas canvas) {
        int first = getFirstVisiblePosition();
        int count = getChildCount();
        int start = -1;
        for (int i = 0; i <= count; i++) {
            boolean row = i < count && grouped(first + i);
            if (row && start < 0) start = i;
            if (!row && start >= 0) {
                drawGroup(canvas, first, start, i - 1);
                start = -1;
            }
        }
        super.dispatchDraw(canvas);
    }

    private void drawGroup(Canvas canvas, int first, int from, int to) {
        View top = getChildAt(from);
        View bottom = getChildAt(to);
        // A group running past the list's edge keeps its corners out of sight.
        float y0 = top.getTop() - (grouped(first + from - 1) ? radius * 2 : 0);
        float y1 = bottom.getBottom() + (grouped(first + to + 1) ? radius * 2 : 0);
        box.set(top.getLeft(), y0, top.getRight(), y1);
        canvas.drawRoundRect(box, radius, radius, fill);
        float half = rim.getStrokeWidth() / 2;
        box.inset(half, half);
        canvas.drawRoundRect(box, radius, radius, rim);
        for (int i = from + 1; i <= to; i++) {
            View row = getChildAt(i);
            canvas.drawRect(row.getLeft() + inset, row.getTop(), row.getRight() - inset, row.getTop() + half * 2, hairline);
        }
    }
}
