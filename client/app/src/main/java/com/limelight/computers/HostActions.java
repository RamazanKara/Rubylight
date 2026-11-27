package com.limelight.computers;

import android.app.Activity;
import android.content.Context;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.widget.Toast;

import androidx.annotation.VisibleForTesting;
import androidx.appcompat.app.AlertDialog;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.PlatformBinding;
import com.limelight.console.HostWaker;
import com.limelight.mic.MicOnlyActivity;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.KnownUpdates;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePcPower;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingLink;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.nvstream.jni.NightjarHostsJni;
import com.limelight.nvstream.jni.NightjarHttpJni;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.nvstream.wol.WakeRelayKeys;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.preferences.LinkGate;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.utils.UiHelper;

import java.io.IOException;
import java.util.concurrent.Executor;
import java.util.function.LongSupplier;

/**
 * Host actions bound to a screen and its {@link ComputerManagerService.ComputerManagerBinder}:
 * pairing, wake, sleep, PC power, host updates, join co-op, rename, pin, unpair, and remove.
 * Pure decision/routing logic lives in {@code nightjar_core::hosts} / {@link HostFlowRoutes}.
 */
public final class HostActions {
    public interface Callbacks {
        ComputerManagerService.ComputerManagerBinder binder();
        boolean isForeground();
        void onPaired(ComputerDetails computer);
        void onRemoved(ComputerDetails computer);
        void onRenamed(ComputerDetails computer);
        void onWakeSent(ComputerDetails computer);
        default void onWakeEnded(ComputerDetails computer) {}
        default void pairingChanged() {}
        void pausePolling();
        void resumePolling();
    }

    private final Activity activity;
    private final Callbacks callbacks;

    private Object pairingRequest;
    private Thread pairingThread;
    private volatile int pairingGeneration;
    private AlertDialog pairingDialog;
    private Runnable pendingPairingResult;

    private final Handler main = new Handler(Looper.getMainLooper());
    private Route sleepRoute;
    private AlertDialog sleepProgress;
    private Route powerRoute;
    private AlertDialog powerProgress;
    private Route updateRoute;
    private HostUpdateFlow updateFlow;
    private AlertDialog updateDialog;
    private CoopSheet coopSheet;
    private HostWaker waker;
    private ComputerDetails wakeComputer;
    private AwayWakeFlow awayWake;
    private AlertDialog errorDialog;
    private final java.util.List<AlertDialog> trackedDialogs = new java.util.ArrayList<>();

    public HostActions(Activity activity, Callbacks callbacks) {
        this.activity = activity;
        this.callbacks = callbacks;
    }

    Activity activity() { return activity; }
    boolean gone() { return activity.isFinishing() || activity.isDestroyed(); }
    public boolean isPairing() { return pairingThread != null; }

    public boolean deliverPendingResult() {
        Runnable result = pendingPairingResult;
        pendingPairingResult = null;
        if (result == null) return false;
        result.run();
        return true;
    }

    public void destroy() {
        cancelPairing();
        pendingPairingResult = null;
        endWake();
        if (sleepRoute != null) { sleepRoute.detach(); sleepRoute = null; }
        dismissSleepProgress();
        if (powerRoute != null) { powerRoute.detach(); powerRoute = null; }
        dismissPowerProgress();
        if (updateRoute != null) { updateRoute.detach(); updateRoute = null; }
        updateFlow = null;
        dismissUpdateDialog();
        closeCoop();
        if (errorDialog != null) { errorDialog.dismiss(); errorDialog = null; }
        for (AlertDialog dialog : new java.util.ArrayList<>(trackedDialogs)) {
            if (dialog.isShowing()) {
                dialog.dismiss();
            }
        }
        trackedDialogs.clear();
    }

    private AlertDialog trackDialog(AlertDialog dialog) {
        if (dialog != null) {
            trackedDialogs.removeIf(existing -> !existing.isShowing());
            trackedDialogs.add(dialog);
        }
        return dialog;
    }

    private ComputerManagerService.ComputerManagerBinder binderForOnline(
            ComputerDetails computer, int title, boolean allowUnknown) {
        boolean badState = allowUnknown
                ? computer.state == ComputerDetails.State.OFFLINE
                : computer.state != ComputerDetails.State.ONLINE;
        if (badState || computer.activeAddress == null) {
            showError(title, activity.getString(allowUnknown ? R.string.error_pc_offline : R.string.error_pc_offline), null);
            return null;
        }
        return requireBinder(title);
    }

    private ComputerManagerService.ComputerManagerBinder requireBinder(int title) {
        ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(title, activity.getString(R.string.error_manager_not_running), null);
        }
        return binder;
    }

    // Pairing

    public void pair(ComputerDetails computer) { pair(computer, null, null, null); }
    public void pairWithLink(ComputerDetails computer, PairingLink link) { pair(computer, link.otp, link.passphrase, link); }

    public void pairWithOneTimePin(final ComputerDetails computer) {
        trackDialog(HostFlowRoutes.showPairOneTimePinDialog(activity, computer,
                (pinValue, passphraseValue) -> pair(computer, pinValue, passphraseValue, null)));
    }

    public synchronized void cancelPairing() {
        pairingGeneration++;
        pendingPairingResult = null;
        if (pairingRequest instanceof Long handle) {
            NightjarHttpJni.clientCancelPendingRequests(handle);
            pairingRequest = null;
            LimeLog.info("Pairing cancelled");
        }
        if (pairingThread != null) { pairingThread.interrupt(); pairingThread = null; }
        if (pairingDialog != null) { pairingDialog.dismiss(); pairingDialog = null; }
        LinkGate.release(this, activity);
    }

    private void pair(final ComputerDetails computer, final String oneTimePin, final String passphrase,
                      final PairingLink link) {
        if (computer.state == ComputerDetails.State.OFFLINE || computer.activeAddress == null) {
            showPairingFailure(activity.getString(R.string.pair_pc_offline), null);
            return;
        }
        final ComputerManagerService.ComputerManagerBinder binder = requireBinder(R.string.pair_pairing_title);
        if (binder == null) return;
        cancelPairing();
        LinkGate.hold(this);
        final int generation = pairingGeneration;
        final String pin = oneTimePin == null ? NightjarHostsJni.randomPairingPin() : oneTimePin;
        pairingDialog = com.limelight.ui.PairingCinemaSheet.show(activity, computer.name, oneTimePin, pin, () -> {
            cancelPairing();
            callbacks.resumePolling();
        });
        callbacks.pausePolling();
        LimeLog.info("Pairing started");
        pairingThread = new Thread(() -> HostFlowRoutes.runPairWorker(
                activity, this, binder, computer, link, pin, oneTimePin, passphrase, generation), "Pairing");
        pairingThread.start();
    }

    synchronized boolean acceptPairingRequest(int generation, Object request) {
        if (generation != pairingGeneration) return false;
        pairingRequest = request;
        return true;
    }

    @VisibleForTesting static boolean refused(int status) { return HostFlowRoutes.refused(status); }
    @VisibleForTesting static int refusalMessage(boolean fromLink, boolean oneTimePin) {
        return HostFlowRoutes.refusalMessage(fromLink, oneTimePin);
    }
    @VisibleForTesting static int failureMessage(PairState state, boolean fromLink, boolean oneTimePin, boolean inGame) {
        return HostFlowRoutes.failureMessage(state, fromLink, oneTimePin, inGame);
    }
    @VisibleForTesting static boolean alreadyPaired(PairState state, PairingLink link, java.security.cert.X509Certificate pinned) {
        return HostFlowRoutes.alreadyPaired(state, link, pinned);
    }

    @VisibleForTesting
    void showPairingResult(int generation, boolean paired, String failure,
                           ComputerDetails computer, String oneTimePin) {
        showPairingResult(generation, paired, failure, computer, oneTimePin, false);
    }

    void showPairingResult(int generation, boolean paired, String failure,
                           ComputerDetails computer, String oneTimePin, boolean fromLink) {
        if (generation != pairingGeneration || gone()) return;
        if (!callbacks.isForeground()) {
            pendingPairingResult = () -> showPairingResult(generation, paired, failure, computer, oneTimePin, fromLink);
            return;
        }
        pairingRequest = null;
        pairingThread = null;
        if (pairingDialog != null) { pairingDialog.dismiss(); pairingDialog = null; }
        LimeLog.info(paired ? "Pairing succeeded" : "Pairing failed");
        if (paired) {
            callbacks.onPaired(computer);
        } else {
            callbacks.resumePolling();
            showPairingFailure(failure, fromLink ? null : () -> {
                if (oneTimePin == null) pair(computer); else pairWithOneTimePin(computer);
            });
        }
        LinkGate.release(this, activity);
    }

    // Wake

    public void wake(final ComputerDetails computer) {
        final Context app = activity.getApplicationContext();
        wake(computer, pc -> {
                    WakeRelayKeys.Kept kept = new WakeRelayKeys(app).load(pc.uuid);
                    return new AwayWake.Seen(kept, kept != null && AwayWake.onPcNetwork(app, pc));
                }, host -> WakeOnLanSender.sendWolPacket(app, host), task -> new Thread(task, "Wake on LAN").start(),
                AwayWakeFlow.JNI, task -> new Thread(task, "Wake relay").start(), main::post,
                SystemClock::elapsedRealtime);
    }

    /**
     * Today's wake for a PC that is not offline. For an offline one, its away wake keys, its saved addresses and this
     * device's interfaces are read on {@code worker} (a file, preferences and the network stack: never on the main
     * thread), and the path is chosen back on {@code main}. A look that fails leaves today's wake.
     */
    @VisibleForTesting
    void wake(ComputerDetails computer, AwayWake.Look look, HostWaker.Sender sender, Executor io,
              AwayWakeFlow.Relay relay, Executor worker, Executor main, LongSupplier clock) {
        if (computer.state != ComputerDetails.State.OFFLINE) {
            wake(computer, null, false, sender, io, relay, worker, clock);
            return;
        }
        worker.execute(() -> {
            AwayWake.Seen seen;
            try {
                seen = look.at(computer);
            } catch (RuntimeException e) {
                LimeLog.warning("Wake from away cannot read what it needs for " + computer.uuid + ": " + e);
                seen = new AwayWake.Seen(null, false);
            }
            final AwayWake.Seen decided = seen;
            main.execute(() -> {
                if (!gone()) wake(computer, decided.kept, decided.onPcNetwork, sender, io, relay, worker, clock);
            });
        });
    }

    /**
     * An offline PC that offers waking from away, seen from outside its network ({@link AwayWake#path}), gets the
     * magic packets and one of its wake relays at once. Elsewhere today's wake runs; should its wait end with the PC
     * still away, a relay kept for it is asked next (a network away may number itself like the home one), and away
     * without a relay the failure says how to get one.
     */
    @VisibleForTesting
    void wake(ComputerDetails computer, WakeRelayKeys.Kept kept, boolean onPcNetwork, HostWaker.Sender sender,
              Executor io, AwayWakeFlow.Relay relay, Executor worker, LongSupplier clock) {
        AwayWake.Path path = AwayWake.path(computer.state == ComputerDetails.State.OFFLINE, onPcNetwork, kept);
        Runnable askRelay = kept == null || !kept.hasRelay() ? null
                : () -> wakeThroughRelay(computer, kept, sender, io, relay, worker, clock);
        if (path == AwayWake.Path.RELAY) {
            askRelay.run();
            return;
        }
        String noRelay = path == AwayWake.Path.NO_RELAY ? AwayWake.noRelay(computer.displayName()).in(activity) : null;
        startWake(computer, sender, io, clock, askRelay, noRelay);
    }

    /** The magic packets once more, and one of the PC's wake relays asked, with its progress and outcome. */
    private void wakeThroughRelay(ComputerDetails computer, WakeRelayKeys.Kept kept, HostWaker.Sender sender,
                                  Executor io, AwayWakeFlow.Relay relay, Executor worker, LongSupplier clock) {
        endWake();
        if (WakeTargets.hasMac(activity, computer)) {
            final ComputerDetails target = new ComputerDetails(computer);
            io.execute(() -> {
                try {
                    sender.send(target);
                } catch (IOException e) {
                    LimeLog.info("Wake: no packet went out to " + target.uuid + " from away: " + e);
                }
            });
        }
        // Try again asks a relay at once; a PC that answers by now gets today's wake, which says so.
        awayWake = new AwayWakeFlow(this, callbacks, computer, kept, relay, worker,
                () -> wake(computer, kept, false, sender, io, relay, worker, clock));
        awayWake.start();
    }

    @VisibleForTesting
    void startWake(final ComputerDetails computer, HostWaker.Sender sender, Executor io, LongSupplier clock) {
        startWake(computer, sender, io, clock, null, null);
    }

    /**
     * Today's wake: the magic packets until the PC answers or the wait ends. Should it end with the PC still away,
     * {@code askRelay} runs next when there is one, else the failure ends with {@code noRelay} when there is one.
     */
    private void startWake(final ComputerDetails computer, HostWaker.Sender sender, Executor io, LongSupplier clock,
                           Runnable askRelay, String noRelay) {
        if (computer.state == ComputerDetails.State.ONLINE) {
            Toast.makeText(activity, R.string.wol_pc_online, Toast.LENGTH_SHORT).show();
            return;
        }
        if (!WakeTargets.hasMac(activity, computer)) {
            // A relay knows the PC's MACs.
            if (askRelay != null) {
                askRelay.run();
                return;
            }
            showError(R.string.pcview_menu_send_wol, withHint(activity.getString(R.string.wol_no_mac), noRelay), null);
            return;
        }
        endWake();
        final String uuid = computer.uuid;
        final Runnable retry = () -> startWake(computer, sender, io, clock, askRelay, noRelay);
        wakeComputer = computer;
        waker = new HostWaker(main, io, sender, () -> {
            ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
            ComputerDetails live = binder == null ? null : binder.getComputer(uuid);
            return live != null ? live : computer;
        }, clock, new HostWaker.Listener() {
            @Override public void onOnline() {
                LimeLog.info("Wake: " + uuid + " answers");
                callbacks.onWakeEnded(computer);
                if (!gone()) Toast.makeText(activity, R.string.wol_pc_online, Toast.LENGTH_SHORT).show();
            }
            @Override public void onTimedOut() {
                LimeLog.warning("Wake: " + uuid + " did not answer in time");
                callbacks.onWakeEnded(computer);
                if (askRelay != null) {
                    askRelay.run();
                    return;
                }
                String message = withHint(activity.getString(R.string.console_wake_timeout, computer.displayName()),
                        noRelay);
                if (callbacks.isForeground()) {
                    showError(computer.displayName(), message, retry);
                } else if (!gone()) {
                    Toast.makeText(activity, message, Toast.LENGTH_LONG).show();
                }
            }
            @Override public void onSendFailed(IOException error) {
                LimeLog.warning("Wake: no packet went out to " + uuid + ": " + error);
                callbacks.onWakeEnded(computer);
                if (askRelay != null) {
                    askRelay.run();
                    return;
                }
                showError(R.string.pcview_menu_send_wol, withHint(activity.getString(R.string.wol_fail), noRelay),
                        retry);
            }
        });
        callbacks.onWakeSent(computer);
        waker.start();
        if (waker.isWaiting()) Toast.makeText(activity, R.string.wol_waking_msg, Toast.LENGTH_LONG).show();
    }

    private static String withHint(String message, String hint) {
        return hint == null ? message : message + "\n\n" + hint;
    }

    public boolean isWaking() {
        return (waker != null && waker.isWaiting()) || (awayWake != null && awayWake.isWaiting());
    }

    private void endWake() {
        if (awayWake != null) {
            awayWake.cancel();
            awayWake = null;
        }
        if (waker != null) {
            boolean waiting = waker.isWaiting();
            waker.cancel();
            waker = null;
            if (waiting) callbacks.onWakeEnded(wakeComputer);
        }
        wakeComputer = null;
    }

    // Sleep & PC power & update

    public void sleep(final ComputerDetails computer) {
        ComputerManagerService.ComputerManagerBinder binder = binderForOnline(computer, R.string.host_menu_sleep, false);
        if (binder == null) return;
        startSleep(computer, nativeClient(computer, binder), task -> new Thread(task, "Sleep PC").start());
    }

    private PairingDialogs pairingDialogs() {
        return new PairingDialogs(activity, (title, message) -> showError(title, message, null),
                task -> new Thread(task, "Approve device").start(), main::post, callbacks::pairingChanged,
                this::trackDialog);
    }

    public void pairAnother(ComputerDetails computer) {
        ComputerManagerService.ComputerManagerBinder binder = requireBinder(R.string.host_menu_pair_another);
        if (binder != null) pairingDialogs().showRequests(computer, nativeClient(computer, binder));
    }

    public void approvePairing(java.util.List<PairingWatch.Waiting> waiting) {
        ComputerManagerService.ComputerManagerBinder binder = requireBinder(R.string.host_menu_pair_another);
        if (binder == null) return;
        if (waiting.size() == 1) {
            pairingDialogs().showPin(waiting.get(0), nativeClient(waiting.get(0).host, binder));
        } else {
            pairingDialogs().showList(waiting, entry -> nativeClient(entry.host, binder));
        }
    }

    public PairingWatch.Reader pairingReader() {
        return host -> {
            ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
            return binder == null ? java.util.Collections.emptyList()
                    : nativeClient(host, binder).open().pairingRequests();
        };
    }

    private SleepFlow.Client nativeClient(ComputerDetails computer, ComputerManagerService.ComputerManagerBinder binder) {
        final Context app = activity.getApplicationContext();
        return () -> {
            HostEndpoint endpoint = HostEndpoint.of(computer, binder.getUniqueId(),
                    PlatformBinding.getCryptoProvider(app));
            if (endpoint == null) {
                throw new NativeApiException(NativeApiException.Kind.NOT_PAIRED,
                        "No address or pinned certificate for the PC");
            }
            return new NativeApiClient(new JniNativeApiTransport(), endpoint);
        };
    }

    @VisibleForTesting
    void startSleep(ComputerDetails computer, SleepFlow.Client client, Executor worker) {
        endSleep();
        Route route = new Route(activity.getApplicationContext(), this, "Sleep PC");
        sleepRoute = route;
        showSleepProgress();
        new SleepFlow(client, HostFlowRoutes.sleepUi(computer, route), worker, main::post,
                SystemClock::elapsedRealtime, System::currentTimeMillis).start();
    }

    private void endSleep() {
        if (sleepRoute != null) { sleepRoute.drop(); sleepRoute = null; }
        dismissSleepProgress();
    }

    private void showSleepProgress() {
        dismissSleepProgress();
        if (gone()) return;
        sleepProgress = new NavigationDialogBuilder(activity).setTitle(R.string.host_menu_sleep)
                .setMessage(R.string.host_sleep_asking)
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> endSleep())
                .setOnCancelListener(dialog -> endSleep()).show();
    }

    void dismissSleepProgress() {
        if (sleepProgress != null) { sleepProgress.dismiss(); sleepProgress = null; }
    }

    void showSleepConfirm(final ComputerDetails computer, final Runnable sleep) {
        if (gone()) return;
        trackDialog(new NavigationDialogBuilder(activity).startOnCancel()
                .setTitle(activity.getString(R.string.host_sleep_title, computer.displayName()))
                .setMessage(R.string.host_sleep_message)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.host_menu_sleep, (dialog, which) -> {
                    showSleepProgress();
                    sleep.run();
                })
                .show());
    }

    @VisibleForTesting
    SleepFlow.Ui sleepUi(final ComputerDetails computer) {
        return HostFlowRoutes.sleepUi(computer, new Route(activity.getApplicationContext(), this, "Sleep PC"));
    }

    public void pcPower(final ComputerDetails computer, NativePcPower.Action action) {
        ComputerManagerService.ComputerManagerBinder binder =
                binderForOnline(computer, HostFlowRoutes.powerTitle(action), false);
        if (binder == null) return;
        startPcPower(computer, action, nativeClient(computer, binder), task -> new Thread(task, "PC power").start());
    }

    @VisibleForTesting
    void startPcPower(ComputerDetails computer, NativePcPower.Action action, SleepFlow.Client client, Executor worker) {
        startPcPower(computer, action, client, worker, Thread::sleep);
    }

    @VisibleForTesting
    void startPcPower(ComputerDetails computer, NativePcPower.Action action, SleepFlow.Client client, Executor worker,
                      HostUpdateFlow.Sleeper sleeper) {
        endPcPower();
        Route route = new Route(activity.getApplicationContext(), this, "PC power " + action);
        powerRoute = route;
        showPowerProgress(HostFlowRoutes.powerTitle(action), false);
        new PcPowerFlow(action, client, pcPowerUi(computer, action, route), worker, main::post,
                SystemClock::elapsedRealtime, System::currentTimeMillis, sleeper).start();
    }

    private void endPcPower() {
        if (powerRoute != null) { powerRoute.drop(); powerRoute = null; }
        dismissPowerProgress();
    }

    void showPowerProgress(int title, boolean spent) {
        dismissPowerProgress();
        if (gone()) return;
        MaterialAlertDialogBuilder b = new NavigationDialogBuilder(activity).setTitle(title).setMessage(R.string.host_sleep_asking);
        powerProgress = (spent ? b.setPositiveButton(android.R.string.ok, null)
                : b.setNegativeButton(android.R.string.cancel, (d, w) -> endPcPower()).setOnCancelListener(d -> endPcPower()))
                .show();
    }

    void dismissPowerProgress() {
        if (powerProgress != null) { powerProgress.dismiss(); powerProgress = null; }
    }

    void confirmWithCancelFocused(String title, String message, int confirm, Runnable go) {
        if (gone()) return;
        AlertDialog dialog = new NavigationDialogBuilder(activity).startOnCancel()
                .setTitle(title)
                .setMessage(message)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(confirm, (shown, which) -> go.run())
                .create();
        dialog.setOnShowListener(shown -> dialog.getButton(AlertDialog.BUTTON_NEGATIVE).requestFocus());
        dialog.show();
        trackDialog(dialog);
    }

    @VisibleForTesting
    static PcPowerFlow.Ui pcPowerUi(final ComputerDetails computer, final NativePcPower.Action action, final Route route) {
        return HostFlowRoutes.pcPowerUi(computer, action, route);
    }

    public void updateHost(final ComputerDetails computer) {
        String version = KnownUpdates.offered(computer.uuid);
        if (version == null) {
            showError(R.string.host_menu_update, activity.getString(R.string.host_update_none), null);
            return;
        }
        ComputerManagerService.ComputerManagerBinder binder = binderForOnline(computer, R.string.host_menu_update, false);
        if (binder == null) return;
        confirmUpdate(computer, version, () -> startUpdate(computer, nativeClient(computer, binder),
                task -> new Thread(task, "Host update").start(), Thread::sleep));
    }

    @VisibleForTesting
    void confirmUpdate(ComputerDetails computer, String version, Runnable install) {
        confirmWithCancelFocused(activity.getString(R.string.host_update_title, computer.displayName(), version),
                activity.getString(R.string.host_update_message), R.string.host_update_action, install);
    }

    @VisibleForTesting
    void startUpdate(ComputerDetails computer, SleepFlow.Client client, Executor worker, HostUpdateFlow.Sleeper sleeper) {
        endUpdate();
        Route route = new Route(activity.getApplicationContext(), this, "Host update");
        updateRoute = route;
        HostUpdateFlow flow = new HostUpdateFlow(client, updateUi(computer, route), worker, main::post, sleeper);
        updateFlow = flow;
        showUpdateDialog(activity.getString(R.string.host_menu_update), activity.getString(R.string.host_sleep_asking));
        flow.start();
    }

    private void endUpdate() {
        if (updateFlow != null) { updateFlow.cancel(); updateFlow = null; }
        if (updateRoute != null) { updateRoute.drop(); updateRoute = null; }
        dismissUpdateDialog();
    }

    void showUpdateDialog(String title, String message) {
        dismissUpdateDialog();
        if (gone()) return;
        updateDialog = new NavigationDialogBuilder(activity).setTitle(title).setMessage(message)
                .setPositiveButton(android.R.string.ok, null).show();
    }

    void dismissUpdateDialog() {
        if (updateDialog != null) { updateDialog.dismiss(); updateDialog = null; }
    }

    @VisibleForTesting
    static String powerConfirmText(Context context, NativePcPower.Action action, boolean canWake) {
        return HostFlowRoutes.powerConfirmText(context, action, canWake);
    }

    @VisibleForTesting
    static HostUpdateFlow.Ui updateUi(final ComputerDetails computer, final Route route) {
        return HostFlowRoutes.updateUi(computer, route);
    }

    static final class Route extends HostFlowRoutes.Route {
        Route(Context app, HostActions screen, String what) {
            super(app, screen, what);
        }
    }

    // Mic only, co-op, unpair, rename, pin, remove

    public void micOnly(final ComputerDetails computer) {
        ComputerManagerService.ComputerManagerBinder binder = requireBinder(R.string.mic_only_action);
        if (binder == null) return;
        if (computer.state != ComputerDetails.State.ONLINE
                || !MicOnlyActivity.start(activity, computer, binder.getUniqueId())) {
            showError(R.string.mic_only_action, activity.getString(R.string.error_pc_offline), null);
        }
    }

    public void joinCoop(final ComputerDetails computer) {
        ComputerManagerService.ComputerManagerBinder binder = binderForOnline(computer, R.string.host_menu_coop, false);
        if (binder == null) return;
        openCoop(computer, nativeClient(computer, binder)::open, task -> new Thread(task, "Join co-op").start());
    }

    @VisibleForTesting
    CoopSheet openCoop(ComputerDetails computer, CoopSheet.Client client, Executor worker) {
        closeCoop();
        if (gone()) return null;
        coopSheet = new CoopSheet(activity, computer, client, worker, main::post,
                HostFlowRoutes.coopUi(activity, this, callbacks, computer));
        coopSheet.show();
        return coopSheet;
    }

    private void closeCoop() {
        if (coopSheet != null) { coopSheet.close(); coopSheet = null; }
    }

    public void confirmUnpair(final ComputerDetails computer) {
        if (gone()) return;
        trackDialog(new NavigationDialogBuilder(activity).startOnCancel()
                .setTitle(activity.getString(R.string.host_unpair_title, computer.displayName()))
                .setMessage(R.string.host_unpair_message)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.pcview_menu_unpair_pc, (dialog, which) -> unpair(computer))
                .show());
    }

    @VisibleForTesting
    void unpair(final ComputerDetails computer) {
        ComputerManagerService.ComputerManagerBinder binder = binderForOnline(computer, R.string.pcview_menu_unpair_pc, true);
        if (binder == null) return;
        Toast.makeText(activity, R.string.unpairing, Toast.LENGTH_SHORT).show();
        new Thread(() -> HostFlowRoutes.runUnpairWorker(activity, this, callbacks, binder, computer), "Unpair").start();
    }

    public void rename(final ComputerDetails computer) {
        if (gone()) return;
        trackDialog(HostFlowRoutes.showRenameDialog(activity, computer, typed -> saveName(computer, typed)));
    }

    public void chooseAddress(final ComputerDetails computer) {
        if (gone()) return;
        trackDialog(HostFlowRoutes.showChooseAddressDialog(activity, computer, address -> pinAddress(computer, address)));
    }

    @VisibleForTesting
    void pinAddress(ComputerDetails computer, ComputerDetails.AddressTuple address) {
        ComputerManagerService.ComputerManagerBinder binder = requireBinder(R.string.host_menu_connection_address);
        if (binder == null) return;
        binder.pinAddress(computer.uuid, address);
        computer.pinnedAddress = address;
        if (address != null) computer.activeAddress = address;
    }

    @VisibleForTesting
    void saveName(ComputerDetails computer, String typed) {
        ComputerManagerService.ComputerManagerBinder binder = requireBinder(R.string.host_rename_title);
        if (binder == null) return;
        String alias = HostNames.normalize(typed, computer.name);
        binder.renameComputer(computer.uuid, alias);
        computer.alias = alias;
        callbacks.onRenamed(computer);
    }

    public void confirmRemove(final ComputerDetails computer) {
        if (android.app.ActivityManager.isUserAMonkey()) {
            LimeLog.info("Ignoring delete PC request from monkey");
            return;
        }
        UiHelper.displayDeletePcConfirmationDialog(activity, computer, () -> {
            ComputerManagerService.ComputerManagerBinder binder = requireBinder(R.string.pcview_menu_delete_pc);
            if (binder == null) return;
            binder.removeComputer(computer);
            callbacks.onRemoved(computer);
        }, null);
    }

    void showError(int title, CharSequence message, Runnable retry) {
        showError(activity.getString(title), message, retry);
    }

    public void showError(CharSequence title, CharSequence message, Runnable retry) {
        if (gone()) return;
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(activity).setTitle(title).setMessage(message);
        if (retry == null) {
            builder.setPositiveButton(android.R.string.ok, null);
        } else {
            builder.setNegativeButton(android.R.string.cancel, null)
                    .setPositiveButton(R.string.pair_try_again, (dialog, which) -> retry.run());
        }
        keepError(builder.show());
    }

    /** A pairing that failed says so where its PIN stood, on the pairing panel. */
    private void showPairingFailure(CharSequence message, Runnable retry) {
        if (gone()) return;
        keepError(com.limelight.ui.PairingCinemaSheet.showFailure(
                activity, activity.getString(R.string.pair_pairing_title), message, retry));
    }

    private void keepError(AlertDialog shown) {
        errorDialog = shown;
        shown.setOnDismissListener(ignored -> { if (errorDialog == shown) errorDialog = null; });
    }

    public boolean isShowingError() { return errorDialog != null && errorDialog.isShowing(); }
}
