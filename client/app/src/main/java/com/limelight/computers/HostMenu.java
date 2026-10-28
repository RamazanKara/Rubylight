package com.limelight.computers;

import android.app.Activity;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.grid.PcGridAdapter;
import com.limelight.nativeapi.KnownCapabilities;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.ui.ActionSheet;

import com.google.android.material.button.MaterialButton;

import org.json.JSONException;

import java.util.function.IntConsumer;

/**
 * A PC's options, one list for the PCs screen and for Home. In this order: Open library, Resume and Quit, Join co-op,
 * Shared folders, Pair, one-time PIN, Wake or Sleep, Files, PC settings, Rename, Unpair or Pairing help, then Remove PC
 * last. The screen that shows it decides what each action does; this class only knows what is offered for which
 * state.
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
    public static final int FILES = 18;
    public static final int COOP = 19;

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
        if (offersCoop(details, state)) {
            sheet.addAction(R.drawable.ic_gamepad, activity.getString(R.string.host_menu_coop),
                    activity.getString(R.string.host_menu_coop_note)).setOnClickListener(v -> {
                        sheet.dismiss();
                        onAction.accept(COOP);
                    });
        }
        if (hasSharedFolders(details, state)) {
            add(sheet, onAction, SHARED_FOLDERS, R.drawable.ic_folder, activity.getString(R.string.folders_title));
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
            String label = activity.getString(R.string.host_menu_sleep);
            MaterialButton action = sleep == SleepEntry.ENABLED ? sheet.addAction(R.drawable.ic_power, label)
                    : sheet.addAction(R.drawable.ic_power, label,
                            activity.getString(details.caps.availability(HostCapabilities.Feature.SLEEP).reasonText()));
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
        if (offersFiles(details, state)) {
            add(sheet, onAction, FILES, R.drawable.ic_files, activity.getString(R.string.files_menu));
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
     * Join co-op on a paired PC whose host listed {@code coop} the last time its native capabilities were read in this
     * process (the computer manager reads them when a poll first finds the PC online). Hidden until then, and on every
     * other host: whether this device may take a free slot is per session, so the co-op sheet says that.
     */
    static boolean offersCoop(ComputerDetails details, PcGridAdapter.RowState state) {
        boolean ready = state == PcGridAdapter.RowState.RUNNING || state == PcGridAdapter.RowState.READY;
        String known = ready ? KnownCapabilities.of(details.uuid) : null;
        if (known == null) {
            return false;
        }
        try {
            return details.caps.withNativeCapabilities(known).has(HostCapabilities.Feature.COOP);
        } catch (JSONException e) {
            LimeLog.warning("Unreadable native capabilities of " + details.uuid + ": " + e.getMessage());
            return false;
        }
    }

    /**
     * Shared folders on a paired, online PC with the native API. Whether it shares any with this device takes a
     * request, so the folders screen answers that; serverinfo does not say.
     */
    static boolean hasSharedFolders(ComputerDetails details, PcGridAdapter.RowState state) {
        return (state == PcGridAdapter.RowState.RUNNING || state == PcGridAdapter.RowState.READY)
                && details.caps.has(HostCapabilities.Feature.NATIVE_API);
    }

    /**
     * Files on a paired PC whose host speaks the native API. Whether this device may send or download is known
     * only once the screen asked the PC, so the screen says what is missing; stock hosts never show the entry.
     */
    static boolean offersFiles(ComputerDetails details, PcGridAdapter.RowState state) {
        boolean ready = state == PcGridAdapter.RowState.RUNNING || state == PcGridAdapter.RowState.READY;
        return ready && details.caps.has(HostCapabilities.Feature.NATIVE_API);
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
