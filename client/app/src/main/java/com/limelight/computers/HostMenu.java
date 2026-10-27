package com.limelight.computers;

import android.app.Activity;

import com.limelight.R;
import com.limelight.grid.PcGridAdapter;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.ui.ActionSheet;

import com.google.android.material.button.MaterialButton;

import java.util.function.IntConsumer;

/**
 * A PC's options, one list for the PCs screen and for Home. In this order: Open library, Resume and Quit,
 * Shared folders, Pair, one-time PIN, Wake or Sleep, PC settings, Rename, Unpair or Pairing help, then Remove PC last. The
 * screen that shows it decides what each action does; this class only knows what is offered for which state.
 */
public final class HostMenu {
    /** How the Sleep entry appears. */
    enum SleepEntry { HIDDEN, ENABLED, DISABLED }

    /** Material's opacity for disabled content, on a row that stays focusable. */
    static final float DISABLED_ALPHA = 0.38f;

    public static final int PAIR = 2;
    public static final int UNPAIR = 3;
    public static final int WAKE = 4;
    public static final int REMOVE = 5;
    public static final int RESUME = 6;
    public static final int QUIT = 7;
    public static final int LIBRARY = 8;
    public static final int PC_SETTINGS = 12;
    public static final int OTP_PAIR = 13;
    public static final int PAIRING_HELP = 14;
    public static final int RENAME = 15;
    public static final int SLEEP = 16;
    public static final int SHARED_FOLDERS = 17;

    private HostMenu() {
    }

    /**
     * The sheet for {@code details}, not shown yet. {@code onAction} gets one of the ids above after the sheet
     * has closed. {@code runningGame} is the name of the game on the PC when the library knows it.
     */
    public static ActionSheet build(Activity activity, ComputerDetails details, PcGridAdapter.RowState state,
                                    String runningGame, IntConsumer onAction) {
        ActionSheet sheet = new ActionSheet(activity, details.displayName());
        boolean ready = state == PcGridAdapter.RowState.RUNNING || state == PcGridAdapter.RowState.READY;
        if (ready) {
            add(sheet, onAction, LIBRARY, R.drawable.ic_library, activity.getString(R.string.library_open));
        }
        if (state == PcGridAdapter.RowState.RUNNING) {
            add(sheet, onAction, RESUME, R.drawable.ic_play, runningGame != null
                    ? activity.getString(R.string.pc_menu_resume_game, runningGame)
                    : activity.getString(R.string.applist_menu_resume));
            if (details.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
                add(sheet, onAction, QUIT, R.drawable.ic_stop, runningGame != null
                        ? activity.getString(R.string.pc_menu_quit_game, runningGame)
                        : activity.getString(R.string.applist_menu_quit));
            }
        }
        if (hasSharedFolders(details, state)) {
            sheet.addAction(R.drawable.ic_folder, activity.getString(R.string.folders_title),
                    activity.getString(R.string.host_menu_folders_note)).setOnClickListener(v -> {
                        sheet.dismiss();
                        onAction.accept(SHARED_FOLDERS);
                    });
        }
        if (state == PcGridAdapter.RowState.NOT_PAIRED) {
            add(sheet, onAction, PAIR, R.drawable.ic_lock, activity.getString(R.string.pcview_menu_pair_pc));
            if (details.caps.has(HostCapabilities.Feature.OTP_PAIRING)) {
                add(sheet, onAction, OTP_PAIR, R.drawable.ic_keyboard, activity.getString(R.string.pair_otp_title));
            }
        }
        if (details.state != ComputerDetails.State.ONLINE && WakeTargets.hasMac(activity, details)) {
            add(sheet, onAction, WAKE, R.drawable.ic_power, activity.getString(R.string.pcview_menu_send_wol));
        } else if (state == PcGridAdapter.RowState.OFFLINE) {
            // Wake needs the PC's address from a pairing made while it was on.
            sheet.addNote(sheet.addGroup(), activity.getString(R.string.host_wake_needs_pairing), false);
        }
        SleepEntry sleep = sleepEntry(details);
        if (sleep != SleepEntry.HIDDEN) {
            MaterialButton action = sheet.addAction(R.drawable.ic_power, activity.getString(R.string.host_menu_sleep),
                    activity.getString(sleep == SleepEntry.ENABLED ? R.string.host_menu_sleep_note :
                            details.caps.availability(HostCapabilities.Feature.SLEEP).reasonText()));
            if (sleep == SleepEntry.DISABLED) {
                // Dimmed but still reachable by D-pad and TalkBack: choosing it asks the host again, which
                // says where to allow it, or offers Sleep if it was allowed since.
                action.setAlpha(DISABLED_ALPHA);
            }
            action.setOnClickListener(v -> {
                sheet.dismiss();
                onAction.accept(SLEEP);
            });
        }
        add(sheet, onAction, PC_SETTINGS, R.drawable.ic_settings, activity.getString(R.string.pc_menu_settings));
        add(sheet, onAction, RENAME, R.drawable.ic_edit, activity.getString(R.string.host_menu_rename));
        if (ready) {
            add(sheet, onAction, UNPAIR, R.drawable.ic_unpair, activity.getString(R.string.pcview_menu_unpair_pc));
        }
        if (details.pairState != PairState.PAIRED) {
            add(sheet, onAction, PAIRING_HELP, R.drawable.ic_help, activity.getString(R.string.discovery_help));
        }
        sheet.addDivider();
        sheet.addDestructiveAction(R.drawable.ic_delete, activity.getString(R.string.pcview_menu_delete_pc))
                .setOnClickListener(v -> {
                    sheet.dismiss();
                    onAction.accept(REMOVE);
                });
        return sheet;
    }

    /**
     * Sleep while the PC is online and its host offers Sleep PC; dimmed with the reason when the host withholds
     * it from this device. Hidden otherwise, also while the host is unknown: a stock host has no such action.
     */
    static SleepEntry sleepEntry(ComputerDetails details) {
        if (details.state != ComputerDetails.State.ONLINE) {
            return SleepEntry.HIDDEN;
        }
        HostCapabilities.Availability sleep = details.caps.availability(HostCapabilities.Feature.SLEEP);
        if (sleep.isSupported()) {
            return SleepEntry.ENABLED;
        }
        return sleep.reason == HostCapabilities.Reason.SLEEP_PERMISSION ? SleepEntry.DISABLED : SleepEntry.HIDDEN;
    }

    /**
     * Shared folders on a paired, online PC with the native API. Whether it shares any with this device takes a
     * request, so the folders screen answers that; serverinfo does not say.
     */
    static boolean hasSharedFolders(ComputerDetails details, PcGridAdapter.RowState state) {
        return (state == PcGridAdapter.RowState.RUNNING || state == PcGridAdapter.RowState.READY)
                && details.caps.has(HostCapabilities.Feature.NATIVE_API);
    }

    private static void add(ActionSheet sheet, IntConsumer onAction, int action, int icon, String label) {
        sheet.addAction(icon, label).setOnClickListener(v -> {
            sheet.dismiss();
            onAction.accept(action);
        });
    }

    /** The running game as its library lists it, or else one with the host's own ID and UUID. */
    public static NvApp runningApp(NvApp known, ComputerDetails details) {
        if (known != null) {
            return known;
        }
        NvApp app = new NvApp("app", details.runningGameId, false);
        app.setAppUuid(details.runningGameUuid);
        return app;
    }
}
