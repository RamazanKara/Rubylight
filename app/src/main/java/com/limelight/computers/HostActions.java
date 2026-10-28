package com.limelight.computers;

import android.app.Activity;
import android.content.Context;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.text.InputFilter;
import android.text.InputType;
import android.view.View;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.widget.EditText;
import android.widget.Toast;

import androidx.annotation.VisibleForTesting;
import androidx.appcompat.app.AlertDialog;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.PlatformBinding;
import com.limelight.console.HostWaker;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingLink;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;

import org.xmlpull.v1.XmlPullParserException;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.net.UnknownHostException;
import java.util.concurrent.Executor;
import java.util.function.LongSupplier;

/**
 * What a person can do to a PC on the list besides opening it: pair (with a PIN shown here or a
 * one-time PIN typed here), wake, sleep, rename, unpair and remove. It belongs to one screen and its binder.
 *
 * <p>Each pairing gets a generation. Cancelling, starting another pairing or leaving the screen
 * bumps it, and a result that carries an older one is dropped, so a pairing that was cancelled never
 * shows a dialog or opens a library. A result that arrives while the screen is in the background
 * waits for its return.
 *
 * <p>Failures are dialogs (they have to be read, and a controller can reach them), with Try again
 * where doing it again can help. Successes that need no decision stay toasts.
 */
public final class HostActions {
    public interface Callbacks {
        /** The computer manager, or null while it is not bound. */
        ComputerManagerService.ComputerManagerBinder binder();

        boolean isForeground();

        /** A pairing succeeded; the screen opens the PC's library. */
        void onPaired(ComputerDetails computer);

        /** The PC was removed from this device. */
        void onRemoved(ComputerDetails computer);

        /** The PC's name on this device changed. */
        void onRenamed(ComputerDetails computer);

        /** A wake signal went out; the row says "Waking…" for a while. */
        void onWakeSent(ComputerDetails computer);

        /** The wait for a woken PC ended: it answered, it did not in time, or no packet went out. */
        default void onWakeEnded(ComputerDetails computer) {
        }

        void pausePolling();

        void resumePolling();
    }

    private final Activity activity;
    private final Callbacks callbacks;

    private NvHTTP pairingRequest;
    private Thread pairingThread;
    private volatile int pairingGeneration;
    private AlertDialog pairingDialog;
    // A pairing result that arrived while the screen was in the background, shown on return.
    private Runnable pendingPairingResult;

    private final Handler main = new Handler(Looper.getMainLooper());
    // The newest Sleep PC flow, and its progress while the host is asked.
    private SleepRoute sleepRoute;
    private AlertDialog sleepProgress;
    // The PC being woken and waited for.
    private HostWaker waker;
    private ComputerDetails wakeComputer;

    public HostActions(Activity activity, Callbacks callbacks) {
        this.activity = activity;
        this.callbacks = callbacks;
    }

    private boolean gone() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    /** True from the moment a pairing starts until its result has been shown or it was cancelled. */
    public boolean isPairing() {
        return pairingThread != null;
    }

    /** Shows the result that waited for the screen's return. Returns false when nothing waited. */
    public boolean deliverPendingResult() {
        Runnable result = pendingPairingResult;
        pendingPairingResult = null;
        if (result == null) {
            return false;
        }
        result.run();
        return true;
    }

    /**
     * The screen is going away: nothing a pairing finds out later may reach it, and a running Sleep PC flow
     * lets go of it and reports by toast.
     */
    public void destroy() {
        cancelPairing();
        pendingPairingResult = null;
        endWake();
        if (sleepRoute != null) {
            sleepRoute.detach();
            sleepRoute = null;
        }
        dismissSleepProgress();
    }

    // Pairing

    public void pair(ComputerDetails computer) {
        pair(computer, null, null, null);
    }

    /**
     * Pairs with a QR pairing link (Experimental): its one-time PIN, refused when the PC's certificate
     * is not the link's. A failure offers no Try again, since the link's PIN works once.
     */
    public void pairWithLink(ComputerDetails computer, PairingLink link) {
        pair(computer, link.otp, link.passphrase, link);
    }

    public void pairWithOneTimePin(final ComputerDetails computer) {
        View fields = activity.getLayoutInflater().inflate(R.layout.dialog_pair_otp, null);
        EditText pin = fields.findViewById(R.id.pair_pin);
        pin.setInputType(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_VARIATION_PASSWORD);
        pin.setFilters(new InputFilter[] {new InputFilter.LengthFilter(4)});
        pin.setSaveEnabled(false);
        EditText passphrase = fields.findViewById(R.id.pair_passphrase);
        passphrase.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        passphrase.setSingleLine(true);
        passphrase.setSaveEnabled(false);
        AlertDialog dialog = new NavigationDialogBuilder(activity)
                .setTitle(R.string.pair_otp_title)
                .setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.pcview_menu_pair_pc, null)
                .create();
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            String pinValue = pin.getText().toString();
            String passphraseValue = passphrase.getText().toString();
            if (!pinValue.matches("[0-9]{4}")) {
                pin.setError(activity.getString(R.string.pair_otp_invalid_pin));
                return;
            }
            if (passphraseValue.codePointCount(0, passphraseValue.length()) < 4) {
                passphrase.setError(activity.getString(R.string.pair_otp_invalid_passphrase));
                return;
            }
            dialog.dismiss();
            pair(computer, pinValue, passphraseValue, null);
        }));
        dialog.setOnDismissListener(ignored -> {
            pin.getText().clear();
            passphrase.getText().clear();
        });
        dialog.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        dialog.show();
    }

    public synchronized void cancelPairing() {
        pairingGeneration++;
        pendingPairingResult = null;
        if (pairingRequest != null) {
            pairingRequest.cancelPendingRequests();
            pairingRequest = null;
            LimeLog.info("Pairing cancelled");
        }
        if (pairingThread != null) {
            pairingThread.interrupt();
            pairingThread = null;
        }
        if (pairingDialog != null) {
            pairingDialog.dismiss();
            pairingDialog = null;
        }
    }

    private void pair(final ComputerDetails computer, final String oneTimePin, final String passphrase,
                      final PairingLink link) {
        if (computer.state == ComputerDetails.State.OFFLINE || computer.activeAddress == null) {
            showError(R.string.pair_pairing_title, activity.getString(R.string.pair_pc_offline), null);
            return;
        }
        final ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(R.string.pair_pairing_title, activity.getString(R.string.error_manager_not_running), null);
            return;
        }
        cancelPairing();
        final int generation = pairingGeneration;
        final String pin = oneTimePin == null ? PairingManager.generatePinString() : oneTimePin;
        android.text.SpannableStringBuilder pairingText = new android.text.SpannableStringBuilder();
        if (oneTimePin == null) {
            // One instruction, then the PIN; the dialog used to ask for the PIN twice.
            pairingText.append(activity.getString(R.string.pair_pairing_help)).append("\n\n");
            int pinStart = pairingText.length();
            pairingText.append(pin);
            pairingText.setSpan(new android.text.style.RelativeSizeSpan(2f), pinStart, pairingText.length(), 0);
            pairingText.setSpan(new android.text.style.StyleSpan(android.graphics.Typeface.BOLD), pinStart, pairingText.length(), 0);
        } else {
            pairingText.append(activity.getString(R.string.pairing));
        }
        pairingDialog = new NavigationDialogBuilder(activity).setTitle(R.string.pair_pairing_title)
                .setMessage(pairingText)
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> {
                    cancelPairing();
                    callbacks.resumePolling();
                }).setOnCancelListener(dialog -> {
                    cancelPairing();
                    callbacks.resumePolling();
                }).create();
        pairingDialog.show();
        callbacks.pausePolling();
        LimeLog.info("Pairing started");
        pairingThread = new Thread(() -> {
            String message = null;
            boolean success = false;
            try {
                binder.waitForPollingStopped();
                NvHTTP request = new NvHTTP(ServerHelper.getCurrentAddressFromComputer(computer),
                        computer.httpsPort, binder.getUniqueId(), computer.serverCert,
                        PlatformBinding.getCryptoProvider(activity.getApplicationContext()));
                synchronized (this) {
                    if (generation != pairingGeneration) {
                        request.cancelPendingRequests();
                        return;
                    }
                    pairingRequest = request;
                }
                if (alreadyPaired(request.getPairState(), link, computer.serverCert)) {
                    // Already paired, with the certificate pinned then: a link naming another one is refused.
                    if (link != null && !link.matches(computer.serverCert)) {
                        message = activity.getString(R.string.pair_qr_cert_mismatch);
                    } else {
                        success = true;
                    }
                } else {
                    PairingManager pairing = request.getPairingManager();
                    PairState state = link != null ? pairing.pair(request.getServerInfo(true), link)
                            : pairing.pair(request.getServerInfo(true), pin, passphrase);
                    if (state == PairState.PAIRED) {
                        ComputerDetails stored = binder.getComputer(computer.uuid);
                        if (stored != null) {
                            stored.serverCert = pairing.getPairedCert();
                            binder.invalidateStateForComputer(computer.uuid);
                            success = true;
                        } else {
                            message = activity.getString(R.string.pair_fail);
                        }
                    } else if (state == PairState.CERT_MISMATCH) {
                        message = activity.getString(R.string.pair_qr_cert_mismatch);
                    } else {
                        message = activity.getString(state == PairState.PIN_WRONG ?
                                (link != null ? R.string.pair_qr_incorrect_pin :
                                        oneTimePin == null ? R.string.pair_incorrect_pin : R.string.pair_otp_incorrect_pin) :
                                state == PairState.ALREADY_IN_PROGRESS ? R.string.pair_already_in_progress :
                                        computer.runningGameId != 0 ? R.string.pair_pc_ingame : R.string.pair_fail);
                    }
                }
            } catch (IOException | XmlPullParserException e) {
                message = activity.getString(R.string.pair_connection_fix);
            }
            final boolean paired = success;
            final String failure = message;
            activity.runOnUiThread(() -> showPairingResult(generation, paired, failure, computer, oneTimePin, link != null));
        }, "Pairing");
        pairingThread.start();
    }

    /**
     * True when the host already knows this device and nothing is to be paired. A link pairs again while no
     * certificate is pinned, so its fingerprint is checked and the certificate pinned.
     */
    @VisibleForTesting
    static boolean alreadyPaired(PairState state, PairingLink link, java.security.cert.X509Certificate pinned) {
        return state == PairState.PAIRED && (link == null || pinned != null);
    }

    @VisibleForTesting
    void showPairingResult(int generation, boolean paired, String failure,
                           ComputerDetails computer, String oneTimePin) {
        showPairingResult(generation, paired, failure, computer, oneTimePin, false);
    }

    private void showPairingResult(int generation, boolean paired, String failure,
                                   ComputerDetails computer, String oneTimePin, boolean fromLink) {
        if (generation != pairingGeneration || gone()) {
            return;
        }
        if (!callbacks.isForeground()) {
            pendingPairingResult = () -> showPairingResult(generation, paired, failure, computer, oneTimePin, fromLink);
            return;
        }
        pairingRequest = null;
        pairingThread = null;
        if (pairingDialog != null) {
            pairingDialog.dismiss();
            pairingDialog = null;
        }
        LimeLog.info(paired ? "Pairing succeeded" : "Pairing failed");
        if (paired) {
            callbacks.onPaired(computer);
        } else {
            callbacks.resumePolling();
            showError(R.string.pair_pairing_title, failure, fromLink ? null : () -> {
                if (oneTimePin == null) {
                    pair(computer);
                } else {
                    pairWithOneTimePin(computer);
                }
            });
        }
    }

    // Wake

    /**
     * Wakes the PC and waits for it: the packets go out again while it stays away, and it is told when the PC answers
     * or does not within the wait. One PC at a time; waking another moves the wait to it.
     */
    public void wake(final ComputerDetails computer) {
        final Context app = activity.getApplicationContext();
        startWake(computer, host -> WakeOnLanSender.sendWolPacket(app, host),
                task -> new Thread(task, "Wake on LAN").start(), SystemClock::elapsedRealtime);
    }

    @VisibleForTesting
    void startWake(final ComputerDetails computer, HostWaker.Sender sender, Executor io, LongSupplier clock) {
        if (computer.state == ComputerDetails.State.ONLINE) {
            Toast.makeText(activity, R.string.wol_pc_online, Toast.LENGTH_SHORT).show();
            return;
        }
        if (!WakeTargets.hasMac(activity, computer)) {
            showError(R.string.pcview_menu_send_wol, activity.getString(R.string.wol_no_mac), null);
            return;
        }
        endWake();
        final String uuid = computer.uuid;
        wakeComputer = computer;
        waker = new HostWaker(main, io, sender, () -> {
            // The manager's object follows the polls; without it the PC is never seen online and the wait times out.
            ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
            ComputerDetails live = binder == null ? null : binder.getComputer(uuid);
            return live != null ? live : computer;
        }, clock, new HostWaker.Listener() {
            @Override
            public void onOnline() {
                LimeLog.info("Wake: " + uuid + " answers");
                callbacks.onWakeEnded(computer);
                if (!gone()) Toast.makeText(activity, R.string.wol_pc_online, Toast.LENGTH_SHORT).show();
            }

            @Override
            public void onTimedOut() {
                LimeLog.warning("Wake: " + uuid + " did not answer in time");
                callbacks.onWakeEnded(computer);
                String message = activity.getString(R.string.console_wake_timeout, computer.displayName());
                if (callbacks.isForeground()) {
                    showError(computer.displayName(), message, () -> startWake(computer, sender, io, clock));
                } else if (!gone()) {
                    Toast.makeText(activity, message, Toast.LENGTH_LONG).show();
                }
            }

            @Override
            public void onSendFailed(IOException error) {
                LimeLog.warning("Wake: no packet went out to " + uuid + ": " + error);
                callbacks.onWakeEnded(computer);
                showError(R.string.pcview_menu_send_wol, activity.getString(R.string.wol_fail),
                        () -> startWake(computer, sender, io, clock));
            }
        });
        // Told before the start: a PC the manager already sees online ends the wait in it, and that end then
        // follows this, so no "Waking…" or poll for it stays behind.
        callbacks.onWakeSent(computer);
        waker.start();
        if (waker.isWaiting()) {
            Toast.makeText(activity, R.string.wol_waking_msg, Toast.LENGTH_LONG).show();
        }
    }

    /** Whether a wake started here still waits for its PC. */
    public boolean isWaking() {
        return waker != null && waker.isWaiting();
    }

    /** Cancels the wake; a PC still waited for is told its wait ended, so it says "Waking…" no more. */
    private void endWake() {
        if (waker != null) {
            boolean waiting = waker.isWaiting();
            waker.cancel();
            waker = null;
            if (waiting) {
                callbacks.onWakeEnded(wakeComputer);
            }
        }
        wakeComputer = null;
    }

    // Sleep

    /**
     * Sleep PC: the host hands out a token, the person confirms, and the token puts the PC to sleep. The host
     * refuses while a game or stream runs on it; its own words are shown then. The host keeps one token per
     * client, so only the newest flow speaks: a newer one, Cancel or leaving the screen ends the older one.
     */
    public void sleep(final ComputerDetails computer) {
        if (computer.state != ComputerDetails.State.ONLINE || computer.activeAddress == null) {
            showError(R.string.host_menu_sleep, activity.getString(R.string.error_pc_offline), null);
            return;
        }
        final ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(R.string.host_menu_sleep, activity.getString(R.string.error_manager_not_running), null);
            return;
        }
        final Context app = activity.getApplicationContext();
        startSleep(computer, () -> {
            HostEndpoint endpoint = HostEndpoint.of(computer, binder.getUniqueId(),
                    PlatformBinding.getCryptoProvider(app));
            if (endpoint == null) {
                throw new NativeApiException(NativeApiException.Kind.NOT_PAIRED,
                        "No address or pinned certificate for the PC");
            }
            return new NativeApiClient(new JniNativeApiTransport(), endpoint);
        }, task -> new Thread(task, "Sleep PC").start());
    }

    @VisibleForTesting
    void startSleep(ComputerDetails computer, SleepFlow.Client client, Executor worker) {
        endSleep();
        SleepRoute route = new SleepRoute(activity.getApplicationContext(), sleepUi(computer));
        sleepRoute = route;
        showSleepProgress();
        new SleepFlow(client, route, worker, main::post, SystemClock::elapsedRealtime).start();
    }

    /** Ends the current Sleep PC flow here: what it finds out later is logged and dropped. */
    private void endSleep() {
        if (sleepRoute != null) {
            sleepRoute.drop();
            sleepRoute = null;
        }
        dismissSleepProgress();
    }

    private void showSleepProgress() {
        dismissSleepProgress();
        if (gone()) {
            return;
        }
        sleepProgress = new NavigationDialogBuilder(activity).setTitle(R.string.host_menu_sleep)
                .setMessage(R.string.host_sleep_asking)
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> endSleep())
                .setOnCancelListener(dialog -> endSleep())
                .show();
    }

    private void dismissSleepProgress() {
        if (sleepProgress != null) {
            sleepProgress.dismiss();
            sleepProgress = null;
        }
    }

    /** What the screen shows for an offer without a token: why, from the host's flags, and where to allow it. */
    static String sleepUnavailableText(Context context, NativeApiClient.PowerReply offer) {
        if (!offer.sleepSupported) {
            return context.getString(R.string.host_sleep_unsupported);
        }
        if (!offer.sleepAllowed) {
            return context.getString(R.string.host_feature_reason_sleep_permission) + "\n\n"
                    + context.getString(R.string.host_sleep_permission_help);
        }
        return context.getString(R.string.host_sleep_busy);
    }

    /** The host's own words, or the status when it sent none. */
    static String sleepRefusedText(Context context, String message, int httpStatus) {
        return message.isEmpty() ? context.getString(R.string.host_sleep_refused, httpStatus) : message;
    }

    /** Whether asking again can help: only when the PC could not be reached. */
    static boolean sleepRetries(NativeApiException error) {
        return error.getKind() == NativeApiException.Kind.NETWORK || error.getKind() == NativeApiException.Kind.TLS;
    }

    /** Unreachable, unpaired, or the native API's own words for anything else. */
    static String sleepFailedText(Context context, NativeApiException error) {
        if (sleepRetries(error)) {
            return context.getString(R.string.host_sleep_failed);
        }
        if (error.getKind() == NativeApiException.Kind.NOT_PAIRED) {
            return context.getString(R.string.host_feature_reason_pair);
        }
        return context.getString(R.string.host_sleep_error, error.getMessage());
    }

    @VisibleForTesting
    SleepFlow.Ui sleepUi(final ComputerDetails computer) {
        final String title = activity.getString(R.string.host_menu_sleep);
        return new SleepFlow.Ui() {
            @Override
            public void confirm(Runnable sleep) {
                dismissSleepProgress();
                if (gone()) return;
                new NavigationDialogBuilder(activity)
                        .setTitle(activity.getString(R.string.host_sleep_title, computer.displayName()))
                        .setMessage(R.string.host_sleep_message)
                        .setNegativeButton(android.R.string.cancel, null)
                        .setPositiveButton(R.string.host_menu_sleep, (dialog, which) -> {
                            showSleepProgress();
                            sleep.run();
                        })
                        .show();
            }

            @Override
            public void accepted() {
                dismissSleepProgress();
                if (!gone()) Toast.makeText(activity, R.string.host_sleep_accepted, Toast.LENGTH_LONG).show();
            }

            @Override
            public void unavailable(NativeApiClient.PowerReply offer) {
                dismissSleepProgress();
                boolean busy = offer.sleepSupported && offer.sleepAllowed;
                showError(title, sleepUnavailableText(activity, offer), busy ? () -> sleep(computer) : null);
            }

            @Override
            public void refused(String message, int httpStatus) {
                dismissSleepProgress();
                // 409: busy or a spent token; asking again helps once the PC is idle.
                showError(title, sleepRefusedText(activity, message, httpStatus),
                        httpStatus == 409 ? () -> sleep(computer) : null);
            }

            @Override
            public void failed(NativeApiException error) {
                dismissSleepProgress();
                showError(title, sleepFailedText(activity, error), sleepRetries(error) ? () -> sleep(computer) : null);
            }
        };
    }

    /**
     * Carries one Sleep PC flow's outcomes, on the main thread: to its screen; once the screen is gone, as a
     * toast, so a result is never lost; once a newer flow or Cancel ended it, nowhere. It keeps only the
     * application context after the screen goes, so a flow that outlives its screen does not hold the activity.
     */
    static final class SleepRoute implements SleepFlow.Ui {
        private final Context app;
        private SleepFlow.Ui screen;
        private boolean dropped;

        SleepRoute(Context app, SleepFlow.Ui screen) {
            this.app = app;
            this.screen = screen;
        }

        /** The screen went away: outcomes become toasts. */
        void detach() {
            screen = null;
        }

        /** A newer flow or Cancel took over: outcomes are only logged. */
        void drop() {
            dropped = true;
            screen = null;
        }

        private boolean speaks(String outcome) {
            if (dropped) {
                LimeLog.info("Sleep PC " + outcome + " after it was replaced or cancelled; not shown");
            }
            return !dropped;
        }

        private void toast(String text) {
            Toast.makeText(app, text, Toast.LENGTH_LONG).show();
        }

        @Override
        public void confirm(Runnable sleep) {
            LimeLog.info("Sleep PC offered a token");
            if (!speaks("offer")) return;
            if (screen != null) {
                screen.confirm(sleep);
            } else {
                // Nothing was asked of the PC yet; sleeping it unconfirmed is no option.
                LimeLog.info("Sleep PC not confirmed: its screen closed first");
            }
        }

        @Override
        public void accepted() {
            LimeLog.info("Sleep PC accepted");
            if (!speaks("acceptance")) return;
            if (screen != null) {
                screen.accepted();
            } else {
                toast(app.getString(R.string.host_sleep_accepted));
            }
        }

        @Override
        public void unavailable(NativeApiClient.PowerReply offer) {
            LimeLog.warning("Sleep PC offered no token: supported=" + offer.sleepSupported +
                    " allowed=" + offer.sleepAllowed + " available=" + offer.sleepAvailable);
            if (!speaks("answer")) return;
            if (screen != null) {
                screen.unavailable(offer);
            } else {
                toast(sleepUnavailableText(app, offer));
            }
        }

        @Override
        public void refused(String message, int httpStatus) {
            LimeLog.warning("Sleep PC refused: HTTP " + httpStatus + " " + message);
            if (!speaks("refusal")) return;
            if (screen != null) {
                screen.refused(message, httpStatus);
            } else {
                toast(sleepRefusedText(app, message, httpStatus));
            }
        }

        @Override
        public void failed(NativeApiException error) {
            LimeLog.warning("Sleep PC failed: " + error.getKind() + " " + error.getMessage());
            if (!speaks("failure")) return;
            if (screen != null) {
                screen.failed(error);
            } else {
                toast(sleepFailedText(app, error));
            }
        }
    }

    // Unpair

    /** Unpair asks the PC to drop this device, so it asks first; the PC stays in the list. */
    public void confirmUnpair(final ComputerDetails computer) {
        new NavigationDialogBuilder(activity)
                .setTitle(activity.getString(R.string.host_unpair_title, computer.displayName()))
                .setMessage(R.string.host_unpair_message)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.pcview_menu_unpair_pc, (dialog, which) -> unpair(computer))
                .show();
    }

    @VisibleForTesting
    void unpair(final ComputerDetails computer) {
        if (computer.state == ComputerDetails.State.OFFLINE || computer.activeAddress == null) {
            showError(R.string.pcview_menu_unpair_pc, activity.getString(R.string.error_pc_offline), null);
            return;
        }
        final ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(R.string.pcview_menu_unpair_pc, activity.getString(R.string.error_manager_not_running), null);
            return;
        }
        Toast.makeText(activity, R.string.unpairing, Toast.LENGTH_SHORT).show();
        new Thread(() -> {
            String failure = null;
            boolean retry = true;
            boolean unpaired = false;
            try {
                NvHTTP httpConn = new NvHTTP(ServerHelper.getCurrentAddressFromComputer(computer),
                        computer.httpsPort, binder.getUniqueId(), computer.serverCert,
                        PlatformBinding.getCryptoProvider(activity));
                if (httpConn.getPairState() == PairingManager.PairState.PAIRED) {
                    httpConn.unpair();
                    if (httpConn.getPairState() == PairingManager.PairState.NOT_PAIRED) {
                        unpaired = true;
                        // Drop the cached pair state so the next poll reads it afresh.
                        binder.invalidateStateForComputer(computer.uuid);
                    } else {
                        failure = activity.getString(R.string.unpair_fail);
                    }
                } else {
                    failure = activity.getString(R.string.unpair_error);
                    retry = false;
                }
            } catch (UnknownHostException e) {
                failure = activity.getString(R.string.error_unknown_host);
            } catch (FileNotFoundException e) {
                failure = activity.getString(R.string.error_404);
            } catch (XmlPullParserException | IOException e) {
                // The exception text is for the log; people get the same sentence for every failure.
                LimeLog.warning("Unpair failed: " + e);
                failure = activity.getString(R.string.unpair_fail);
            }
            final boolean done = unpaired;
            final String message = failure;
            final boolean canRetry = retry;
            activity.runOnUiThread(() -> {
                if (done) {
                    // The PC no longer lists this device, so what was read from it is stale.
                    ConsoleRepository.forgetHost(activity, computer.uuid);
                    if (!gone()) Toast.makeText(activity, R.string.unpair_success, Toast.LENGTH_LONG).show();
                } else if (callbacks.isForeground() && !gone()) {
                    showError(R.string.pcview_menu_unpair_pc, message, canRetry ? () -> unpair(computer) : null);
                } else {
                    Toast.makeText(activity, message, Toast.LENGTH_LONG).show();
                }
            });
        }, "Unpair").start();
    }

    // Rename

    /** Asks for a name for the PC. It is kept on this device; empty brings back the PC's own name. */
    public void rename(final ComputerDetails computer) {
        View fields = activity.getLayoutInflater().inflate(R.layout.dialog_rename_host, null);
        EditText name = fields.findViewById(R.id.rename_name);
        name.setFilters(new InputFilter[] {new InputFilter.LengthFilter(HostNames.MAX_LENGTH)});
        // Empty brings back the PC's own name, which the field shows as its placeholder.
        ((com.google.android.material.textfield.TextInputLayout) fields.findViewById(R.id.rename_field))
                .setPlaceholderText(computer.name);
        name.setSaveEnabled(false);
        name.setText(computer.displayName());
        name.setSelection(name.getText().length());
        AlertDialog dialog = new NavigationDialogBuilder(activity)
                .setTitle(R.string.host_rename_title)
                .setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.host_rename_save, (d, which) -> saveName(computer, name.getText().toString()))
                .create();
        name.setOnEditorActionListener((view, action, event) -> {
            if (action != EditorInfo.IME_ACTION_DONE) {
                return false;
            }
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            return true;
        });
        dialog.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE
                | WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE);
        dialog.show();
    }

    @VisibleForTesting
    void saveName(ComputerDetails computer, String typed) {
        ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(R.string.host_rename_title, activity.getString(R.string.error_manager_not_running), null);
            return;
        }
        String alias = HostNames.normalize(typed, computer.name);
        binder.renameComputer(computer.uuid, alias);
        computer.alias = alias;
        callbacks.onRenamed(computer);
    }

    // Remove

    /** Removing deletes the PC and what this device stored for it; the PC still lists this device. */
    public void confirmRemove(final ComputerDetails computer) {
        if (android.app.ActivityManager.isUserAMonkey()) {
            LimeLog.info("Ignoring delete PC request from monkey");
            return;
        }
        UiHelper.displayDeletePcConfirmationDialog(activity, computer, () -> {
            ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
            if (binder == null) {
                showError(R.string.pcview_menu_delete_pc, activity.getString(R.string.error_manager_not_running), null);
                return;
            }
            binder.removeComputer(computer);
            callbacks.onRemoved(computer);
        }, null);
    }

    // Errors

    private void showError(int title, CharSequence message, Runnable retry) {
        showError(activity.getString(title), message, retry);
    }

    /** A failure as a dialog titled {@code title}: OK, or Try again and Cancel when doing it again can help. */
    public void showError(CharSequence title, CharSequence message, Runnable retry) {
        if (gone()) {
            return;
        }
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(activity).setTitle(title).setMessage(message);
        if (retry == null) {
            builder.setPositiveButton(android.R.string.ok, null);
        } else {
            builder.setNegativeButton(android.R.string.cancel, null)
                    .setPositiveButton(R.string.pair_try_again, (dialog, which) -> retry.run());
        }
        builder.show();
    }
}
