package com.limelight.grid;

import android.content.Context;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.BaseAdapter;
import android.widget.ImageView;
import android.widget.ProgressBar;
import android.widget.TextView;

import com.limelight.R;

import java.util.ArrayList;

public abstract class GenericGridAdapter<T> extends BaseAdapter {
    protected final Context context;
    private int layoutId;
    final ArrayList<T> itemList = new ArrayList<>();
    private final LayoutInflater inflater;

    GenericGridAdapter(Context context, int layoutId) {
        this.context = context;
        this.layoutId = layoutId;

        this.inflater = (LayoutInflater) context.getSystemService(Context.LAYOUT_INFLATER_SERVICE);
    }

    void setLayoutId(int layoutId) {
        if (layoutId != this.layoutId) {
            this.layoutId = layoutId;

            // Force the view to be redrawn with the new layout
            notifyDataSetInvalidated();
        }
    }

    public void clear() {
        itemList.clear();
    }

    @Override
    public int getCount() {
        return itemList.size();
    }

    @Override
    public Object getItem(int i) {
        return itemList.get(i);
    }

    @Override
    public long getItemId(int i) {
        return i;
    }

    public abstract void populateView(View parentView, ImageView imgView, ProgressBar prgView, TextView txtView, ImageView overlayView, T obj);

    @Override
    public View getView(int i, View convertView, ViewGroup viewGroup) {
        if (convertView == null) {
            convertView = inflater.inflate(layoutId, viewGroup, false);
            convertView.setOnHoverListener((card, event) -> {
                card.setHovered(event.getActionMasked() != android.view.MotionEvent.ACTION_HOVER_EXIT);
                return false;
            });
        }

        ImageView imgView = convertView.findViewById(R.id.grid_image);
        ImageView overlayView = convertView.findViewById(R.id.grid_overlay);
        TextView txtView = convertView.findViewById(R.id.grid_text);
        ProgressBar prgView = convertView.findViewById(R.id.grid_spinner);

        if (layoutId == R.layout.app_grid_item) {
            View artwork = (View) imgView.getParent();
            float density = context.getResources().getDisplayMetrics().density;
            artwork.getLayoutParams().height = Math.round((viewGroup.getHeight() > 0 &&
                    viewGroup.getHeight() / density < 400 ? 112 : 224) * density);
        }

        populateView(convertView, imgView, prgView, txtView, overlayView, itemList.get(i));

        View more = convertView.findViewById(R.id.grid_more);
        if (more != null) {
            final View card = convertView;
            more.setContentDescription(context.getString(R.string.library_more_for, itemList.get(i).toString()));
            // Keep a single D-pad stop per card; touch and accessibility users get a visible action.
            more.setFocusable(false);
            more.setOnClickListener(view -> {
                if (viewGroup instanceof android.widget.AdapterView) {
                    android.widget.AdapterView<?> grid = (android.widget.AdapterView<?>) viewGroup;
                    int position = grid.getPositionForView(card);
                    android.widget.AdapterView.OnItemLongClickListener listener = grid.getOnItemLongClickListener();
                    if (position != android.widget.AdapterView.INVALID_POSITION && listener != null) {
                        listener.onItemLongClick(grid, card, position, grid.getItemIdAtPosition(position));
                    }
                }
            });
        }
        return convertView;
    }
}
