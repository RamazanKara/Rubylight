package com.limelight.grid;

import android.content.Context;
import android.view.View;
import android.widget.ImageView;
import android.widget.ProgressBar;
import android.widget.TextView;
import androidx.core.view.ViewCompat;

import com.limelight.PcView;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.preferences.PreferenceConfiguration;

import java.util.Collections;
import java.util.Comparator;

public class PcGridAdapter extends GenericGridAdapter<PcView.ComputerObject> {

    public PcGridAdapter(Context context, PreferenceConfiguration prefs) {
        super(context, getLayoutIdForPreferences(prefs));
    }

    private static int getLayoutIdForPreferences(PreferenceConfiguration prefs) {
        return R.layout.pc_grid_item;
    }

    public void updateLayoutWithPreferences(Context context, PreferenceConfiguration prefs) {
        // This will trigger the view to reload with the new layout
        setLayoutId(getLayoutIdForPreferences(prefs));
    }

    public void addComputer(PcView.ComputerObject computer) {
        itemList.add(computer);
        sortList();
    }

    private void sortList() {
        Collections.sort(itemList, new Comparator<PcView.ComputerObject>() {
            @Override
            public int compare(PcView.ComputerObject lhs, PcView.ComputerObject rhs) {
                return lhs.details.name.compareToIgnoreCase(rhs.details.name);
            }
        });
    }

    public boolean removeComputer(PcView.ComputerObject computer) {
        return itemList.remove(computer);
    }

    @Override
    public void populateView(View parentView, ImageView imgView, ProgressBar prgView, TextView txtView, ImageView overlayView, PcView.ComputerObject obj) {
        imgView.setImageResource(R.drawable.ic_computer);
        boolean checking = obj.details.state == ComputerDetails.State.UNKNOWN;
        imgView.setVisibility(checking ? View.INVISIBLE : View.VISIBLE);
        prgView.setVisibility(checking ? View.VISIBLE : View.GONE);
        txtView.setText(obj.details.name);

        int status;
        if (obj.details.state == ComputerDetails.State.OFFLINE) {
            status = R.string.pc_offline;
            overlayView.setImageResource(R.drawable.ic_pc_offline);
        }
        else if (checking) {
            status = R.string.pc_checking;
            overlayView.setImageResource(R.drawable.ic_computer);
        }
        else if (obj.details.pairState == PairingManager.PairState.NOT_PAIRED) {
            status = R.string.pc_needs_pairing;
            overlayView.setImageResource(R.drawable.ic_lock);
        }
        else if (obj.details.pairState == PairingManager.PairState.PAIRED) {
            status = R.string.pc_online_paired;
            overlayView.setImageResource(R.drawable.ic_check);
        }
        else {
            status = R.string.pc_online;
            overlayView.setImageResource(R.drawable.ic_computer);
        }
        TextView statusView = parentView.findViewById(R.id.grid_status);
        statusView.setText(status);
        TextView action = parentView.findViewById(R.id.grid_actions);
        action.setText(checking || obj.details.state == ComputerDetails.State.OFFLINE ?
                R.string.library_pc_options : obj.details.pairState == PairingManager.PairState.PAIRED ?
                R.string.library_open : R.string.pcview_menu_pair_pc);
        parentView.setContentDescription(context.getString(R.string.library_item_description,
                obj.details.name, action.getText()));
        ViewCompat.setStateDescription(parentView, context.getString(status));
        ViewCompat.replaceAccessibilityAction(parentView,
                androidx.core.view.accessibility.AccessibilityNodeInfoCompat.AccessibilityActionCompat.ACTION_LONG_CLICK,
                context.getString(R.string.pc_accessibility_actions), (view, arguments) -> {
                    if (view.getParent() instanceof android.widget.AdapterView) {
                        android.widget.AdapterView<?> parent = (android.widget.AdapterView<?>) view.getParent();
                        int position = parent.getPositionForView(view);
                        android.widget.AdapterView.OnItemLongClickListener listener = parent.getOnItemLongClickListener();
                        return position != android.widget.AdapterView.INVALID_POSITION && listener != null &&
                                listener.onItemLongClick(parent, view, position, parent.getItemIdAtPosition(position));
                    }
                    return false;
                });
    }
}
