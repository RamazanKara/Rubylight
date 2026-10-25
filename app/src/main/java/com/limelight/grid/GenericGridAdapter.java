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
import com.limelight.ui.AdaptiveGridView;

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

    public abstract void populateView(View parentView, ImageView imgView, ProgressBar prgView, TextView txtView, T obj);

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
        TextView txtView = convertView.findViewById(R.id.grid_text);
        ProgressBar prgView = convertView.findViewById(R.id.grid_spinner);

        boolean appTile = layoutId == R.layout.app_grid_item || layoutId == R.layout.app_grid_item_small;
        resetRecycledState(convertView, imgView, appTile);

        if (appTile) {
            View artwork = (View) imgView.getParent();
            boolean small = layoutId == R.layout.app_grid_item_small;
            boolean shortViewport = viewGroup instanceof AdaptiveGridView && ((AdaptiveGridView) viewGroup).hasShortViewport();
            int width = viewGroup instanceof android.widget.GridView ? ((android.widget.GridView) viewGroup).getColumnWidth() : 0;
            // The art is as wide as the column less the tile's own padding (a TV poster pads itself with its glow).
            if (width > 0) width = Math.max(1, width - convertView.getPaddingLeft() - convertView.getPaddingRight());
            boolean television = (context.getResources().getConfiguration().uiMode & android.content.res.Configuration.UI_MODE_TYPE_MASK)
                    == android.content.res.Configuration.UI_MODE_TYPE_TELEVISION;
            int height = TileSizing.artHeightPx(width, shortViewport, television, small,
                    context.getResources().getDisplayMetrics().density);
            ViewGroup.LayoutParams params = artwork.getLayoutParams();
            if (params.height != height) {
                params.height = height;
                // A recycled card is only re-measured when it asks for a layout.
                artwork.setLayoutParams(params);
            }
        }

        populateView(convertView, imgView, prgView, txtView, itemList.get(i));

        View more = convertView.findViewById(R.id.grid_more);
        if (more != null) {
            final View card = convertView;
            more.setContentDescription(context.getString(R.string.library_more_for, itemList.get(i).toString()));
            // A focusable child would stop the grid from handling taps on the card. D-pad and keyboard users
            // reach the button through AdaptiveGridView; touch and accessibility users tap it directly.
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

    /**
     * A recycled card may still carry the end of its last cover's fade, a scale or offset, or a scale type
     * that was set for another cover. The cover is then drawn off the frame it is sized for, so every bind
     * starts from the layout's own state; the title fallback and the load guard are rebound by populateView.
     */
    static void resetRecycledState(View card, ImageView cover, boolean appTile) {
        cover.clearAnimation();
        // Only the app layouts crop their cover; the other layouts keep the scale type they inflate with.
        // It also drops the letterbox backdrop the last cover left on the frame.
        if (appTile) com.limelight.ui.ArtFit.reset(cover);
        cover.setImageMatrix(null);
        cover.setScaleX(1);
        cover.setScaleY(1);
        cover.setTranslationX(0);
        cover.setTranslationY(0);
        cover.setAlpha(1f);
        View frame = (View) cover.getParent();
        frame.setScaleX(1);
        frame.setScaleY(1);
        frame.setTranslationX(0);
        frame.setTranslationY(0);
        // The focus scale belongs to the card's state; a card that is not selected has none left over.
        if (!card.isSelected() && !card.isFocused()) {
            // The ring, glow and title fade with the state too: a recycled card shows none of them.
            com.limelight.ui.FastTravel.settle(card);
        }
    }
}
