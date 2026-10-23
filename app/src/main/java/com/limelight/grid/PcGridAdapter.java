package com.limelight.grid;

import android.content.Context;
import android.content.res.ColorStateList;
import android.view.View;
import android.widget.AdapterView;
import android.widget.ImageView;
import android.widget.ProgressBar;
import android.widget.TextView;

import androidx.core.view.ViewCompat;
import androidx.core.view.accessibility.AccessibilityNodeInfoCompat.AccessibilityActionCompat;

import com.google.android.material.color.MaterialColors;
import com.limelight.PcView;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.update.HostUpdateChecker;

import java.util.Collections;
import java.util.Comparator;
import java.util.List;

public class PcGridAdapter extends GenericGridAdapter<PcView.ComputerObject> {
    /** What a PC row says, in the order rows are listed. */
    public enum RowState {
        RUNNING, READY, NOT_PAIRED, CHECKING, WAKING, OFFLINE;

        /** Rows sort in groups: running, online and paired, unpaired, then the PCs that don't answer. */
        int group() {
            return this == WAKING ? OFFLINE.ordinal() : ordinal();
        }
    }

    public PcGridAdapter(Context context, PreferenceConfiguration prefs) {
        super(context, R.layout.pc_grid_item);
    }

    public void updateLayoutWithPreferences(Context context, PreferenceConfiguration prefs) {
        setLayoutId(R.layout.pc_grid_item);
    }

    public void addComputer(PcView.ComputerObject computer) {
        itemList.add(computer);
        sortList();
    }

    private void sortList() {
        Collections.sort(itemList, Comparator.comparingInt((PcView.ComputerObject computer) -> computer.rowState().group())
                .thenComparing(computer -> computer.details.displayName(), String.CASE_INSENSITIVE_ORDER));
    }

    public boolean removeComputer(PcView.ComputerObject computer) {
        return itemList.remove(computer);
    }

    @Override
    public void notifyDataSetChanged() {
        // A PC changes group when it starts a game, pairs or stops answering.
        sortList();
        super.notifyDataSetChanged();
    }

    public static RowState rowState(ComputerDetails details, boolean waking) {
        if (details.state == ComputerDetails.State.ONLINE) {
            if (details.pairState != PairingManager.PairState.PAIRED) {
                return RowState.NOT_PAIRED;
            }
            return details.hasRunningGame() ? RowState.RUNNING : RowState.READY;
        }
        if (waking) {
            return RowState.WAKING;
        }
        return details.state == ComputerDetails.State.OFFLINE ? RowState.OFFLINE : RowState.CHECKING;
    }

    /** The game the host reports as running, from its app list, or null when the list doesn't have it. */
    public static NvApp runningApp(List<NvApp> apps, ComputerDetails details) {
        if (apps == null || !details.hasRunningGame()) {
            return null;
        }
        for (NvApp app : apps) {
            if (app.getControl() == NvApp.Control.NONE &&
                    app.matchesRunningApp(details.runningGameId, details.runningGameUuid)) {
                return app;
            }
        }
        return null;
    }

    /** The row's one status line, such as "Playing Racing Game" or "Offline". */
    public static String statusText(Context context, PcView.ComputerObject computer) {
        switch (computer.rowState()) {
            case RUNNING:
                return computer.runningGame != null ?
                        context.getString(R.string.pc_status_playing, computer.runningGame.getAppName()) :
                        context.getString(R.string.pc_status_playing_unknown);
            case READY:
                return context.getString(R.string.pc_status_ready);
            case NOT_PAIRED:
                return context.getString(R.string.pc_needs_pairing);
            case WAKING:
                return context.getString(R.string.pc_status_waking);
            case OFFLINE:
                return context.getString(R.string.pc_offline);
            default:
                return context.getString(R.string.pc_checking);
        }
    }

    /** What tapping the row does, named for screen readers. */
    public static int mainActionLabel(PcView.ComputerObject computer) {
        switch (computer.rowState()) {
            case RUNNING:
            case READY:
                return R.string.library_open;
            case NOT_PAIRED:
                return R.string.pcview_menu_pair_pc;
            case WAKING:
            case OFFLINE:
                return computer.details.macAddress != null ? R.string.pcview_menu_send_wol : R.string.pc_accessibility_actions;
            default:
                return R.string.pc_accessibility_actions;
        }
    }

    @Override
    public void populateView(View parentView, ImageView imgView, ProgressBar prgView, TextView txtView, PcView.ComputerObject obj) {
        RowState state = obj.rowState();
        boolean checking = state == RowState.CHECKING;
        boolean away = state == RowState.OFFLINE || state == RowState.WAKING;
        imgView.setVisibility(checking ? View.INVISIBLE : View.VISIBLE);
        prgView.setVisibility(checking ? View.VISIBLE : View.GONE);
        // A PC that doesn't answer steps back: its icon and name take the quieter colours.
        imgView.setImageTintList(ColorStateList.valueOf(MaterialColors.getColor(imgView, away ?
                com.google.android.material.R.attr.colorOutline : com.google.android.material.R.attr.colorOnSurfaceVariant)));
        txtView.setText(obj.details.displayName());
        txtView.setTextColor(MaterialColors.getColor(txtView, away ?
                com.google.android.material.R.attr.colorOnSurfaceVariant : com.google.android.material.R.attr.colorOnSurface));

        String status = statusText(context, obj);
        TextView statusView = parentView.findViewById(R.id.grid_status);
        statusView.setText(status);
        statusView.setTextColor(MaterialColors.getColor(statusView, state == RowState.RUNNING ?
                androidx.appcompat.R.attr.colorPrimary : com.google.android.material.R.attr.colorOnSurfaceVariant));

        String update = HostUpdateChecker.availableVersion(context, obj.details.caps);
        TextView updateView = parentView.findViewById(R.id.grid_update);
        int updateVisibility = update == null ? View.GONE : View.VISIBLE;
        if (update == null) {
            // GridView advances to the next row using the last card's height.
            for (PcView.ComputerObject computer : itemList) {
                if (HostUpdateChecker.availableVersion(context, computer.details.caps) != null) {
                    updateVisibility = View.INVISIBLE;
                    break;
                }
            }
        }
        updateView.setVisibility(updateVisibility);
        if (update != null) {
            updateView.setText(context.getString(R.string.host_update_available, update));
            status += ". " + updateView.getText();
        }

        parentView.setContentDescription(context.getString(R.string.library_item_description, obj.details.displayName(), status));
        // The grid owns taps and long presses; name both for screen readers and route them back to it.
        ViewCompat.replaceAccessibilityAction(parentView, AccessibilityActionCompat.ACTION_CLICK,
                context.getString(mainActionLabel(obj)), (view, arguments) -> {
                    AdapterView<?> grid = grid(view);
                    int position = grid == null ? AdapterView.INVALID_POSITION : grid.getPositionForView(view);
                    return position != AdapterView.INVALID_POSITION &&
                            grid.performItemClick(view, position, grid.getItemIdAtPosition(position));
                });
        ViewCompat.replaceAccessibilityAction(parentView, AccessibilityActionCompat.ACTION_LONG_CLICK,
                context.getString(R.string.pc_accessibility_actions), (view, arguments) -> openOptions(view));
        // A mouse's right button opens the same options.
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.M) {
            parentView.setOnContextClickListener(PcGridAdapter::openOptions);
        }
    }

    /** Opens the options of the PC on this card, as a long press does. */
    private static boolean openOptions(View card) {
        AdapterView<?> grid = grid(card);
        int position = grid == null ? AdapterView.INVALID_POSITION : grid.getPositionForView(card);
        AdapterView.OnItemLongClickListener listener = grid == null ? null : grid.getOnItemLongClickListener();
        return position != AdapterView.INVALID_POSITION && listener != null &&
                listener.onItemLongClick(grid, card, position, grid.getItemIdAtPosition(position));
    }

    private static AdapterView<?> grid(View card) {
        return card.getParent() instanceof AdapterView ? (AdapterView<?>) card.getParent() : null;
    }
}
