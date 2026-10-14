package com.limelight.grid;

import android.content.Context;
import android.graphics.BitmapFactory;
import android.view.View;
import android.widget.ImageView;
import android.widget.ProgressBar;
import android.widget.TextView;

import com.limelight.AppView;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.grid.assets.CachedAppAssetLoader;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.grid.assets.MemoryAssetLoader;
import com.limelight.grid.assets.NetworkAssetLoader;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;

import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.text.Normalizer;
import java.util.Set;

@SuppressWarnings("unchecked")
public class AppGridAdapter extends GenericGridAdapter<AppView.AppObject> {
    private static final int ART_WIDTH_PX = 300;
    private static final int SMALL_WIDTH_DP = 104;
    private static final int LARGE_WIDTH_DP = 160;

    private final ComputerDetails computer;
    private final String uniqueId;
    private boolean showHiddenApps;
    private String searchQuery = "";

    private CachedAppAssetLoader loader;
    private Set<Integer> hiddenAppIds = new HashSet<>();
    private ArrayList<AppView.AppObject> allApps = new ArrayList<>();

    public AppGridAdapter(Context context, PreferenceConfiguration prefs, ComputerDetails computer, String uniqueId, boolean showHiddenApps) {
        super(context, getLayoutIdForPreferences(prefs));

        this.computer = computer;
        this.uniqueId = uniqueId;
        this.showHiddenApps = showHiddenApps;

        updateLayoutWithPreferences(context, prefs);
    }

    public void updateHiddenApps(Set<Integer> newHiddenAppIds, boolean hideImmediately) {
        this.hiddenAppIds.clear();
        this.hiddenAppIds.addAll(newHiddenAppIds);

        for (AppView.AppObject app : allApps) {
            app.isHidden = hiddenAppIds.contains(app.app.getAppId());
        }
        notifyDataSetChanged();
    }

    public void setSearchQuery(String query) {
        searchQuery = normalizeSearch(query);
        notifyDataSetChanged();
    }

    public void setShowHiddenApps(boolean show) {
        showHiddenApps = show;
        notifyDataSetChanged();
    }

    public int getLibraryCount() {
        int count = 0;
        for (AppView.AppObject app : allApps) {
            if (showHiddenApps || !app.isHidden) count++;
        }
        return count;
    }

    private static String normalizeSearch(String value) {
        return Normalizer.normalize(value == null ? "" : value, Normalizer.Form.NFD)
                .replaceAll("\\p{M}+", "").toLowerCase(Locale.ROOT).trim();
    }

    private boolean isVisible(AppView.AppObject app) {
        return (showHiddenApps || !app.isHidden) &&
                (searchQuery.isEmpty() || normalizeSearch(app.app.getAppName()).contains(searchQuery));
    }

    private static int getLayoutIdForPreferences(PreferenceConfiguration prefs) {
        if (prefs.smallIconMode) {
            return R.layout.app_grid_item_small;
        }
        else {
            return R.layout.app_grid_item;
        }
    }

    public void updateLayoutWithPreferences(Context context, PreferenceConfiguration prefs) {
        int dpi = context.getResources().getDisplayMetrics().densityDpi;
        int dp;

        if (prefs.smallIconMode) {
            dp = SMALL_WIDTH_DP;
        }
        else {
            dp = LARGE_WIDTH_DP;
        }

        double scalingDivisor = ART_WIDTH_PX / (dp * (dpi / 160.0));
        if (scalingDivisor < 1.0) {
            // We don't want to make them bigger before draw-time
            scalingDivisor = 1.0;
        }
        LimeLog.info("Art scaling divisor: " + scalingDivisor);

        if (loader != null) {
            // Cancel operations on the old loader
            cancelQueuedOperations();
        }

        this.loader = new CachedAppAssetLoader(computer, scalingDivisor,
                new NetworkAssetLoader(context, uniqueId),
                new MemoryAssetLoader(),
                new DiskAssetLoader(context),
                BitmapFactory.decodeResource(context.getResources(), R.drawable.no_app_image));

        // This will trigger the view to reload with the new layout
        setLayoutId(getLayoutIdForPreferences(prefs));
    }

    public void cancelQueuedOperations() {
        loader.cancelForegroundLoads();
        loader.cancelBackgroundLoads();
        loader.freeCacheMemory();
    }

    private static void sortList(List<AppView.AppObject> list) {
        Collections.sort(list, new Comparator<AppView.AppObject>() {
            @Override
            public int compare(AppView.AppObject lhs, AppView.AppObject rhs) {
                return lhs.app.compareForDisplay(rhs.app);
            }
        });
    }

    public void addApp(AppView.AppObject app) {
        // Update hidden state
        app.isHidden = hiddenAppIds.contains(app.app.getAppId());

        // Always add the app to the all apps list
        allApps.add(app);
        sortList(allApps);

        // Add the app to the adapter data if it's not hidden
        if (isVisible(app)) {
            // Queue a request to fetch this bitmap into cache
            loader.queueCacheLoad(app.app);

            // Add the app to our sorted list
            itemList.add(app);
            sortList(itemList);
        }
    }

    public void removeApp(AppView.AppObject app) {
        itemList.remove(app);
        allApps.remove(app);
    }

    /** A snapshot of the full catalogue, including apps omitted from the visible grid. */
    public List<AppView.AppObject> getAllApps() {
        return new ArrayList<>(allApps);
    }

    @Override
    public void notifyDataSetChanged() {
        sortList(allApps);
        itemList.clear();
        for (AppView.AppObject app : allApps) {
            if (isVisible(app)) itemList.add(app);
        }
        super.notifyDataSetChanged();
    }

    @Override
    public void clear() {
        super.clear();
        allApps.clear();
    }

    @Override
    public void populateView(View parentView, ImageView imgView, ProgressBar prgView, TextView txtView, AppView.AppObject obj) {
        // Let the cached asset loader handle it
        loader.populateImageView(obj.app, imgView, txtView);

        TextView roleView = parentView.findViewById(R.id.grid_role);
        NvApp.Role role = obj.app.getRole();
        int roleLabel = roleLabel(obj.app);
        roleView.setVisibility(roleLabel == 0 ? View.GONE : View.VISIBLE);
        if (roleLabel != 0) {
            roleView.setText(roleLabel);
            roleView.setCompoundDrawablesRelativeWithIntrinsicBounds(
                    role == NvApp.Role.REMOTE_MONITOR ? R.drawable.ic_remote_monitor : R.drawable.ic_input_only, 0, 0, 0);
        }

        parentView.setActivated(obj.isRunning);
        TextView status = parentView.findViewById(R.id.grid_status);
        status.setText(obj.isRunning ? (obj.isHidden ? R.string.app_running_hidden : R.string.app_running) : R.string.app_hidden);
        status.setVisibility(obj.isRunning || obj.isHidden ? View.VISIBLE : View.INVISIBLE);
        TextView actions = parentView.findViewById(R.id.grid_actions);
        actions.setText(obj.isRunning ? R.string.applist_menu_resume :
                obj.app.getControl() != NvApp.Control.NONE ? R.string.library_action : R.string.library_play);
        actions.setVisibility(View.VISIBLE);
        String description = obj.app.getAppName();
        if (roleLabel != 0) {
            description = context.getString(R.string.library_item_description, description, roleView.getText());
        }
        if (obj.isRunning || obj.isHidden) {
            description = context.getString(R.string.library_item_description, description, status.getText());
        }
        if (actions.getVisibility() == View.VISIBLE) {
            description = context.getString(R.string.library_item_description, description, actions.getText());
        }
        parentView.setContentDescription(description);
    }

    static int runningActionLabel(NvApp app) {
        return app.getControl() == NvApp.Control.NONE ? R.string.app_resume_quit : R.string.applist_menu_resume;
    }

    static int roleLabel(NvApp app) {
        if (app.getControl() == NvApp.Control.DISCONNECT_MONITOR) return R.string.stream_end_monitor;
        if (app.getControl() == NvApp.Control.DISCONNECT_INPUT) return R.string.stream_end_input;
        if (app.getRole() == NvApp.Role.REMOTE_MONITOR) return R.string.role_view_only;
        if (app.getRole() == NvApp.Role.INPUT_ONLY) return R.string.role_input_only;
        return 0;
    }
}
