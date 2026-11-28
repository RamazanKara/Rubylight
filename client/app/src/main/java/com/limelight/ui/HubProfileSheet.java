package com.limelight.ui;

import android.app.Activity;
import android.widget.LinearLayout;

import com.limelight.R;
import com.limelight.nativeapi.HubSyncController;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.Json;
import com.limelight.utils.Dialog;

import java.util.ArrayList;
import java.util.List;

/**
 * Synced Host Hub profiles, host-override badges, and concurrent-edit conflict chooser rendered
 * in Nightjar's zero-blur Material {@link ActionSheet} with full D-pad/TV and TalkBack support.
 */
public final class HubProfileSheet {
    private HubProfileSheet() {}

    /** Opens the synced profiles & host-overrides sheet for {@code hostUuid} (optional {@code appId}). */
    public static void show(Activity activity, ComputerDetails computer, int appId,
                            HubSyncController.HubBackend backend) {
        if (activity == null || computer == null || computer.uuid == null) return;
        HubSyncController.HostState state = HubSyncController.stateFor(activity, computer.uuid);
        ActionSheet sheet = new ActionSheet(activity, activity.getString(R.string.hub_sync_sheet_title));
        sheet.setSubtitle(statusText(activity, state));
        LinearLayout group = sheet.addGroup();

        if (state.status == HubSyncController.SyncStatus.PERMISSION_DENIED) {
            sheet.addNote(group, activity.getString(R.string.hub_sync_permission_denied), true);
        } else if (state.status == HubSyncController.SyncStatus.OFFLINE_DEGRADED && !state.outbox.isEmpty()) {
            sheet.addNote(group, activity.getString(R.string.hub_sync_offline_saved), false);
        }

        if (state.pendingConflict != null) {
            HubSyncController.HubBackend effective =
                    backend != null ? backend : HubSyncController.backendFor(activity, computer);
            addConflictSection(activity, sheet, group, computer.uuid, state.pendingConflict, effective);
        }

        addOverrideSection(activity, sheet, group, state.snapshot, appId);
        addProfilesSection(activity, sheet, group, state.snapshot);

        sheet.show();
    }

    private static String statusText(Activity activity, HubSyncController.HostState state) {
        if (state.pendingConflict != null) {
            return activity.getString(R.string.hub_sync_conflict_title);
        }
        if (state.snapshot != null) {
            return activity.getString(R.string.hub_sync_revision,
                    state.snapshot.optLong("snapshot_revision", 1L));
        }
        return activity.getString(R.string.hub_sync_not_loaded);
    }

    private static void addConflictSection(Activity activity, ActionSheet sheet, LinearLayout group,
                                           String hostUuid, Json.Obj conflict,
                                           HubSyncController.HubBackend backend) {
        sheet.addSection(group, R.string.hub_sync_conflict_title);
        sheet.addNote(group, activity.getString(R.string.hub_sync_conflict_body), true);
        sheet.addAction(group, R.drawable.ic_arrow_back, activity.getString(R.string.hub_sync_keep_mine))
                .setOnClickListener(v -> {
                    HubSyncController.resolveConflictAsync(activity, hostUuid, "keep_mine", backend);
                    sheet.dismiss();
                });
        sheet.addAction(group, R.drawable.ic_monitor, activity.getString(R.string.hub_sync_use_pc))
                .setOnClickListener(v -> {
                    HubSyncController.resolveConflictAsync(activity, hostUuid, "use_host", backend);
                    sheet.dismiss();
                });
        sheet.addAction(group, R.drawable.ic_copy, activity.getString(R.string.hub_sync_save_copy))
                .setOnClickListener(v -> {
                    HubSyncController.resolveConflictAsync(activity, hostUuid, "save_copy", backend);
                    sheet.dismiss();
                });
    }

    private static void addOverrideSection(Activity activity, ActionSheet sheet, LinearLayout group,
                                           Json.Obj snapshot, int appId) {
        if (snapshot == null) return;
        Json.Arr overrides = snapshot.optArray("host_overrides_summary");
        if (overrides == null || overrides.length() == 0) return;
        List<String> keys = new ArrayList<>();
        for (int i = 0; i < overrides.length(); i++) {
            Json.Obj entry = overrides.optObject(i);
            if (entry == null) continue;
            long targetApp = entry.optLong("app_id", 0L);
            if (targetApp == appId || targetApp == 0L) {
                Json.Arr locked = entry.optArray("locked_keys");
                if (locked != null) {
                    for (int j = 0; j < locked.length(); j++) {
                        keys.add(locked.optString(j, ""));
                    }
                }
            }
        }
        if (keys.isEmpty()) return;
        sheet.addSection(group, R.string.hub_override_badge);
        sheet.addAction(group, R.drawable.ic_speed, activity.getString(R.string.hub_override_badge))
                .setOnClickListener(v -> Dialog.displayDialog(
                        activity,
                        activity.getString(R.string.hub_override_badge),
                        activity.getString(R.string.hub_override_explain, String.join(", ", keys)),
                        false));
    }

    private static void addProfilesSection(Activity activity, ActionSheet sheet, LinearLayout group,
                                           Json.Obj snapshot) {
        if (snapshot == null) return;
        Json.Arr profiles = snapshot.optArray("profiles");
        if (profiles == null || profiles.length() == 0) {
            sheet.addNote(group, activity.getString(R.string.hub_sync_no_profiles), false);
            return;
        }
        sheet.addSection(group, R.string.hub_sync_profiles_section);
        for (int i = 0; i < profiles.length(); i++) {
            Json.Obj p = profiles.optObject(i);
            if (p == null) continue;
            String name = p.optString("name", p.optString("id"));
            Json.Obj stream = p.optObject("stream");
            String desc = summarizeStream(activity, stream);
            String label = desc.isEmpty() ? name : name + " · " + desc;
            sheet.addAction(group, R.drawable.ic_monitor, label).setOnClickListener(v -> sheet.dismiss());
        }
    }

    private static String summarizeStream(Activity activity, Json.Obj stream) {
        if (stream == null) return "";
        int w = stream.optInt("width", 0);
        int h = stream.optInt("height", 0);
        int fps = stream.optInt("fps", 0);
        int scale = stream.optInt("resolution_scale_pct", 0);
        StringBuilder sb = new StringBuilder();
        if (w > 0 && h > 0) sb.append(w).append('×').append(h);
        if (fps > 0) {
            if (sb.length() > 0) sb.append(' ');
            sb.append(fps).append(' ').append(activity.getString(R.string.fps_suffix_fps));
        }
        if (scale > 0) {
            if (sb.length() > 0) sb.append(" · ");
            sb.append(scale).append('%');
        }
        return sb.toString();
    }
}
