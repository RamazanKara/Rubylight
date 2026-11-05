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
import com.limelight.mic.MicOnlyActivity;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.KnownUpdates;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePcPower;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingLink;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.nvstream.wol.WakeTargets;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.preferences.LinkGate;
import com.limelight.utils.LaunchSource;
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
 * one-time PIN typed here), wake, sleep, restart, shut down, update the host, join co-op, rename, unpair and remove.
 * It belongs to one screen and its binder.
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

        /** A waiting device was approved or denied from here; the list changed. */
        default void pairingChanged() {
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
    // The newest Restart PC or Shut down PC flow and its progress, and the newest host update with its dialog.
    private Route powerRoute;
    private AlertDialog powerProgress;
    private Route updateRoute;
    private HostUpdateFlow updateFlow;
    private AlertDialog updateDialog;
    // The open Join co-op sheet.
    private CoopSheet coopSheet;
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
        // Restart, Shut down and an update's wait go on and report by toast; their routes let go of this screen, so
        // a wait of hours holds no activity.
        if (powerRoute != null) {
            powerRoute.detach();
            powerRoute = null;
        }
        dismissPowerProgress();
        if (updateRoute != null) {
            updateRoute.detach();
            updateRoute = null;
        }
        updateFlow = null;
        dismissUpdateDialog();
        closeCoop();
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
        LinkGate.release(this, activity);
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
        // A pairing link from outside waits for this pairing instead of coming over its dialog.
        LinkGate.hold(this);
        final int generation = pairingGeneration;
        final String pin = oneTimePin == null ? PairingManager.generatePinString() : oneTimePin;
        android.text.SpannableStringBuilder pairingText = new android.text.SpannableStringBuilder();
        if (oneTimePin == null) {
            // One instruction, then the PIN; the dialog used to ask for the PIN twice.
            pairingText.append(activity.getString(R.string.pair_pairing_help)).append("\n\n");
            int pinStart = pairingText.length();
            pairingText.append(pin);
            pairingText.setSpan(new android.text.style.RelativeSizeSpan(2.25f), pinStart, pairingText.length(), 0);
            pairingText.setSpan(new android.text.style.StyleSpan(android.graphics.Typeface.BOLD), pinStart, pairingText.length(), 0);
            // The PIN is what the user reads off this screen, so it takes the accent colour.
            pairingText.setSpan(new android.text.style.ForegroundColorSpan(com.google.android.material.color.MaterialColors
                    .getColor(activity, androidx.appcompat.R.attr.colorPrimary, android.graphics.Color.WHITE)),
                    pinStart, pairingText.length(), 0);
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
            } catch (HostHttpResponseException e) {
                LimeLog.warning("Pairing failed: " + e.getMessage());
                message = activity.getString(refused(e.getErrorCode()) ? refusalMessage(link != null, oneTimePin != null)
                        : R.string.pair_connection_fix);
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
    /**
     * Whether an error status means the host refused the request (a used or expired code, say) rather than failed
     * to answer it: 4xx, as in the host's XML status. A 503 or another server error keeps the network advice.
     */
    @VisibleForTesting
    static boolean refused(int status) {
        return status >= 400 && status < 500;
    }

    /** What to show when the host refuses a pairing request instead of answering it with a pairing state. */
    @VisibleForTesting
    static int refusalMessage(boolean fromLink, boolean oneTimePin) {
        return fromLink ? R.string.pair_qr_incorrect_pin : oneTimePin ? R.string.pair_otp_incorrect_pin : R.string.pair_fail;
    }

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
        LinkGate.release(this, activity);
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
        startSleep(computer, nativeClient(computer, binder), task -> new Thread(task, "Sleep PC").start());
    }

    // Approving a device

    private PairingDialogs pairingDialogs() {
        return new PairingDialogs(activity, (title, message) -> showError(title, message, null),
                task -> new Thread(task, "Approve device").start(), main::post, callbacks::pairingChanged);
    }

    /** The menu's Pair another device. */
    public void pairAnother(ComputerDetails computer) {
        ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(R.string.host_menu_pair_another, activity.getString(R.string.error_manager_not_running), null);
            return;
        }
        pairingDialogs().showRequests(computer, nativeClient(computer, binder));
    }

    /** Home's banner: one device goes straight to its PIN, more to the list. */
    public void approvePairing(java.util.List<PairingWatch.Waiting> waiting) {
        ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(R.string.host_menu_pair_another, activity.getString(R.string.error_manager_not_running), null);
            return;
        }
        if (waiting.size() == 1) {
            pairingDialogs().showPin(waiting.get(0), nativeClient(waiting.get(0).host, binder));
        } else {
            pairingDialogs().showList(waiting, entry -> nativeClient(entry.host, binder));
        }
    }

    /** Reads a PC's waiting devices on a worker, for {@link PairingWatch}. Without the service there is nothing to read. */
    public PairingWatch.Reader pairingReader() {
        return host -> {
            ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
            return binder == null ? java.util.Collections.emptyList()
                    : nativeClient(host, binder).open().pairingRequests();
        };
    }

    /** The paired PC's native API, opened on the worker. */
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

    // Restart, shut down and update

    /**
     * Restart PC or Shut down PC, as Sleep PC: the host hands out a token, the person confirms with Cancel focused,
     * and a token that is fresh, or fetched again, does it. A refusal shows the host's words, naming what keeps the
     * PC busy. Only the newest flow speaks.
     */
    public void pcPower(final ComputerDetails computer, NativePcPower.Action action) {
        int title = powerTitle(action);
        if (computer.state != ComputerDetails.State.ONLINE || computer.activeAddress == null) {
            showError(title, activity.getString(R.string.error_pc_offline), null);
            return;
        }
        final ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(title, activity.getString(R.string.error_manager_not_running), null);
            return;
        }
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
        showPowerProgress(powerTitle(action), false);
        new PcPowerFlow(action, client, pcPowerUi(computer, action, route), worker, main::post,
                SystemClock::elapsedRealtime, sleeper).start();
    }

    private void endPcPower() {
        if (powerRoute != null) {
            powerRoute.drop();
            powerRoute = null;
        }
        dismissPowerProgress();
    }

    /**
     * While the host is asked. Before the confirmation Cancel ends the flow; once the token is {@code spent} the PC
     * may go down whatever the person taps, so OK only hides the progress and the outcome still shows.
     */
    private void showPowerProgress(int title, boolean spent) {
        dismissPowerProgress();
        if (gone()) {
            return;
        }
        powerProgress = spent
                ? new NavigationDialogBuilder(activity).setTitle(title).setMessage(R.string.host_sleep_asking)
                        .setPositiveButton(android.R.string.ok, null)
                        .show()
                : new NavigationDialogBuilder(activity).setTitle(title).setMessage(R.string.host_sleep_asking)
                        .setNegativeButton(android.R.string.cancel, (dialog, which) -> endPcPower())
                        .setOnCancelListener(dialog -> endPcPower())
                        .show();
    }

    private void dismissPowerProgress() {
        if (powerProgress != null) {
            powerProgress.dismiss();
            powerProgress = null;
        }
    }

    private static int powerTitle(NativePcPower.Action action) {
        return action == NativePcPower.Action.RESTART ? R.string.host_menu_restart : R.string.host_menu_shutdown;
    }

    /** The host's reason, or ours when it sent none. */
    static String powerUnavailableText(Context context, String reason) {
        return reason.isEmpty() ? context.getString(R.string.host_power_busy) : reason;
    }

    /** The confirmation's text: what can block it, and before a shutdown whether the PC may not wake again. */
    static String powerConfirmText(Context context, NativePcPower.Action action, boolean wakeFromOff) {
        String text = context.getString(R.string.host_power_message);
        if (action == NativePcPower.Action.SHUT_DOWN && !wakeFromOff) {
            text += "\n\n" + context.getString(R.string.host_shutdown_no_wake);
        }
        return text;
    }

    private static String powerAcceptedText(Context context, NativePcPower.Action action, String name) {
        return context.getString(action == NativePcPower.Action.RESTART ? R.string.host_restart_accepted
                : R.string.host_shutdown_accepted, name);
    }

    /** A question whose Cancel holds the focus, so a stray press on a D-pad or controller never confirms it. */
    private void confirmWithCancelFocused(String title, String message, int confirm, Runnable go) {
        if (gone()) return;
        AlertDialog dialog = new NavigationDialogBuilder(activity)
                .setTitle(title)
                .setMessage(message)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(confirm, (shown, which) -> go.run())
                .create();
        dialog.setOnShowListener(shown -> dialog.getButton(AlertDialog.BUTTON_NEGATIVE).requestFocus());
        dialog.show();
    }

    /** The outcomes of one Restart or Shut down: static, so all it holds of the screen is {@code route}'s. */
    @VisibleForTesting
    static PcPowerFlow.Ui pcPowerUi(final ComputerDetails computer, final NativePcPower.Action action,
                                    final Route route) {
        final Context app = route.app;
        final String title = app.getString(powerTitle(action));
        final String name = computer.displayName();
        return new PcPowerFlow.Ui() {
            @Override
            public void confirm(boolean wakeFromOff, Runnable go) {
                route.deliver("offer", screen -> {
                    screen.dismissPowerProgress();
                    screen.confirmWithCancelFocused(app.getString(action == NativePcPower.Action.RESTART
                                    ? R.string.host_restart_title : R.string.host_shutdown_title, name),
                            powerConfirmText(app, action, wakeFromOff),
                            action == NativePcPower.Action.RESTART ? R.string.host_restart_action
                                    : R.string.host_shutdown_action,
                            () -> {
                                // The token is spent now: no Cancel that would only hide what the PC does.
                                screen.showPowerProgress(powerTitle(action), true);
                                go.run();
                            });
                }, null);
            }

            @Override
            public void accepted() {
                String text = powerAcceptedText(app, action, name);
                route.deliver("acceptance", screen -> {
                    screen.dismissPowerProgress();
                    if (!screen.gone()) Toast.makeText(screen.activity, text, Toast.LENGTH_LONG).show();
                }, text);
            }

            @Override
            public void unavailable(String reason) {
                String text = powerUnavailableText(app, reason);
                route.deliver("answer: " + reason, screen -> {
                    screen.dismissPowerProgress();
                    screen.showError(title, text, null);
                }, text);
            }

            @Override
            public void refused(String message) {
                route.deliver("refusal: " + message, screen -> {
                    screen.dismissPowerProgress();
                    screen.showError(title, message, null);
                }, message);
            }

            @Override
            public void failed(NativeApiException error) {
                String text = sleepFailedText(app, error);
                route.deliver("failure: " + error.getKind() + " " + error.getMessage(), screen -> {
                    screen.dismissPowerProgress();
                    screen.showError(title, text,
                            sleepRetries(error) ? () -> screen.pcPower(computer, action) : null);
                }, text);
            }

            @Override
            public void refusedLater(String refusal, boolean unconfirmed) {
                boolean restart = action == NativePcPower.Action.RESTART;
                // Not confirmed in time is no failure: the PC may still go down.
                int titleId = unconfirmed
                        ? (restart ? R.string.host_restart_unconfirmed_title : R.string.host_shutdown_unconfirmed_title)
                        : (restart ? R.string.host_restart_refused_title : R.string.host_shutdown_refused_title);
                String laterTitle = app.getString(titleId, name);
                route.deliver((unconfirmed ? "unconfirmed after the answer: " : "refusal after the answer: ") + refusal,
                        screen -> {
                            screen.dismissPowerProgress();
                            screen.showError(laterTitle, refusal, null);
                        }, laterTitle + ": " + refusal);
            }
        };
    }

    /**
     * Update the PC's host to the version its last answer offered: confirm with Cancel focused, then the host
     * installs it and Nightjar waits until GET /update answers on that version. Leaving the screen keeps the wait;
     * its outcome comes as a toast then.
     */
    public void updateHost(final ComputerDetails computer) {
        String version = KnownUpdates.offered(computer.uuid);
        if (version == null) {
            showError(R.string.host_menu_update, activity.getString(R.string.host_update_none), null);
            return;
        }
        if (computer.state != ComputerDetails.State.ONLINE || computer.activeAddress == null) {
            showError(R.string.host_menu_update, activity.getString(R.string.error_pc_offline), null);
            return;
        }
        final ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(R.string.host_menu_update, activity.getString(R.string.error_manager_not_running), null);
            return;
        }
        confirmUpdate(computer, version, () -> startUpdate(computer, nativeClient(computer, binder),
                task -> new Thread(task, "Host update").start(), Thread::sleep));
    }

    @VisibleForTesting
    void confirmUpdate(ComputerDetails computer, String version, Runnable install) {
        confirmWithCancelFocused(activity.getString(R.string.host_update_title, computer.displayName(), version),
                activity.getString(R.string.host_update_message), R.string.host_update_action, install);
    }

    @VisibleForTesting
    void startUpdate(ComputerDetails computer, SleepFlow.Client client, Executor worker,
                     HostUpdateFlow.Sleeper sleeper) {
        endUpdate();
        Route route = new Route(activity.getApplicationContext(), this, "Host update");
        updateRoute = route;
        HostUpdateFlow flow = new HostUpdateFlow(client, updateUi(computer, route), worker, main::post, sleeper);
        updateFlow = flow;
        showUpdateDialog(activity.getString(R.string.host_menu_update), activity.getString(R.string.host_sleep_asking));
        flow.start();
    }

    /** A newer update takes over: the earlier wait stops and its outcome is dropped. */
    private void endUpdate() {
        if (updateFlow != null) {
            updateFlow.cancel();
            updateFlow = null;
        }
        if (updateRoute != null) {
            updateRoute.drop();
            updateRoute = null;
        }
        dismissUpdateDialog();
    }

    /**
     * The update's progress. It has no Cancel: once confirmed the host may install whatever the person taps, so OK only
     * hides it, and the wait goes on to show its outcome.
     */
    private void showUpdateDialog(String title, String message) {
        dismissUpdateDialog();
        if (gone()) {
            return;
        }
        updateDialog = new NavigationDialogBuilder(activity).setTitle(title).setMessage(message)
                .setPositiveButton(android.R.string.ok, null)
                .show();
    }

    private void dismissUpdateDialog() {
        if (updateDialog != null) {
            updateDialog.dismiss();
            updateDialog = null;
        }
    }

    /** The outcomes of one host update: static, so all it holds of the screen is {@code route}'s. */
    @VisibleForTesting
    static HostUpdateFlow.Ui updateUi(final ComputerDetails computer, final Route route) {
        final Context app = route.app;
        final String name = computer.displayName();
        final String uuid = computer.uuid;
        final String failedTitle = app.getString(R.string.host_update_failed_title, name);
        return new HostUpdateFlow.Ui() {
            @Override
            public void started(String version) {
                String text = app.getString(R.string.host_update_started, version);
                route.deliver("start " + version, screen -> screen.showUpdateDialog(
                        app.getString(R.string.host_update_started_title, name), text), null);
            }

            @Override
            public void waiting(String reason) {
                String text = app.getString(R.string.host_update_waiting, reason);
                route.deliver("waiting: " + reason, screen -> screen.showUpdateDialog(
                        app.getString(R.string.host_update_started_title, name), text), null);
            }

            @Override
            public void back(String version) {
                KnownUpdates.forget(uuid);
                String title = app.getString(R.string.host_update_back_title, name);
                String text = app.getString(R.string.host_update_back, version);
                route.deliver("back on " + version, screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(title, text, null);
                }, title);
            }

            @Override
            public void timedOut(String version) {
                String text = app.getString(R.string.host_update_timeout, version);
                route.deliver("timeout", screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, text, null);
                }, text);
            }

            @Override
            public void nothingToInstall() {
                String text = app.getString(R.string.host_update_none);
                route.deliver("nothing to install", screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, text, null);
                }, text);
            }

            @Override
            public void updateFailed(String error) {
                String text = error != null ? error : app.getString(R.string.host_update_failed);
                route.deliver("failed on the PC: " + error, screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, text, null);
                }, text);
            }

            @Override
            public void cameBackOn(String version, String error) {
                String back = app.getString(R.string.host_update_old_version, version);
                String text = error != null ? back + " " + error : back;
                route.deliver("back on the old version " + version, screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, text, null);
                }, text);
            }

            @Override
            public void refused(String message) {
                route.deliver("refusal: " + message, screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, message, null);
                }, message);
            }

            @Override
            public void failed(NativeApiException error) {
                String text = sleepFailedText(app, error);
                route.deliver("failure: " + error.getKind() + " " + error.getMessage(), screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(failedTitle, text, null);
                }, text);
            }
        };
    }

    /**
     * Carries one Restart, Shut down or update flow's outcomes, on the main thread, as {@link SleepRoute} does: to
     * its screen; once the screen is gone, as a toast; once a newer flow or Cancel ended it, nowhere. It is the only
     * way a flow reaches the screen, and lets go of it on {@link #detach}, so a wait that outlives the screen (an
     * update's, for hours) does not keep it.
     */
    static final class Route {
        final Context app;
        private final String what;
        private HostActions screen;
        private boolean dropped;

        Route(Context app, HostActions screen, String what) {
            this.app = app;
            this.screen = screen;
            this.what = what;
        }

        void detach() {
            screen = null;
        }

        void drop() {
            dropped = true;
        }

        /** {@code shown} while the screen is there; else {@code toast}, or only the log when it is null. */
        void deliver(String outcome, java.util.function.Consumer<HostActions> shown, String toast) {
            LimeLog.info(what + " " + outcome);
            if (dropped) {
                LimeLog.info(what + " " + outcome + " after it was replaced or cancelled; not shown");
            } else if (screen != null) {
                shown.accept(screen);
            } else if (toast != null) {
                Toast.makeText(app, toast, Toast.LENGTH_LONG).show();
            }
        }
    }

    // Microphone only

    /** Microphone only: this device's microphone on the PC, with no picture, until Stop in its notification. */
    public void micOnly(final ComputerDetails computer) {
        ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(R.string.mic_only_action, activity.getString(R.string.error_manager_not_running), null);
            return;
        }
        if (computer.state != ComputerDetails.State.ONLINE
                || !MicOnlyActivity.start(activity, computer, binder.getUniqueId())) {
            showError(R.string.mic_only_action, activity.getString(R.string.error_pc_offline), null);
        }
    }

    // Join co-op

    /**
     * Join co-op: the PC's running co-op sessions, and a join that seats this device and starts its stream into the
     * slot. Only the newest sheet speaks: opening another closes the older one.
     */
    public void joinCoop(final ComputerDetails computer) {
        if (computer.state != ComputerDetails.State.ONLINE || computer.activeAddress == null) {
            showError(R.string.host_menu_coop, activity.getString(R.string.error_pc_offline), null);
            return;
        }
        final ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(R.string.host_menu_coop, activity.getString(R.string.error_manager_not_running), null);
            return;
        }
        final Context app = activity.getApplicationContext();
        openCoop(computer, () -> {
            HostEndpoint endpoint = HostEndpoint.of(computer, binder.getUniqueId(),
                    PlatformBinding.getCryptoProvider(app));
            if (endpoint == null) {
                throw new NativeApiException(NativeApiException.Kind.NOT_PAIRED,
                        "No address or pinned certificate for the PC");
            }
            return new NativeApiClient(new JniNativeApiTransport(), endpoint);
        }, task -> new Thread(task, "Join co-op").start());
    }

    @VisibleForTesting
    CoopSheet openCoop(ComputerDetails computer, CoopSheet.Client client, Executor worker) {
        closeCoop();
        if (gone()) {
            return null;
        }
        coopSheet = new CoopSheet(activity, computer, client, worker, main::post, new CoopSheet.Ui() {
            @Override
            public void problem(CharSequence title, CharSequence message, Runnable retry) {
                showError(title, message, retry);
            }

            @Override
            public void start(NvApp app) {
                ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
                if (binder == null) {
                    showError(R.string.host_menu_coop, activity.getString(R.string.error_manager_not_running), null);
                    return;
                }
                ServerHelper.doStart(activity, app, computer, binder, LaunchSource.COOP_JOIN);
            }

            @Override
            public void reopen() {
                joinCoop(computer);
            }
        });
        coopSheet.show();
        return coopSheet;
    }

    private void closeCoop() {
        if (coopSheet != null) {
            coopSheet.close();
            coopSheet = null;
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
                        // Its certificate goes too, so Home does not show it as a paired PC while it is away.
                        binder.forgetCertificate(computer.uuid);
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

    /**
     * Automatic, or one of the PC's known addresses to always connect through (Moonlight V+ PC's address
     * pinning).
     */
    public void chooseAddress(final ComputerDetails computer) {
        java.util.List<ComputerDetails.AddressTuple> known = computer.knownAddresses();
        if (computer.pinnedAddress != null && !known.contains(computer.pinnedAddress)) {
            known.add(computer.pinnedAddress);
        }
        String[] items = new String[known.size() + 1];
        items[0] = activity.getString(R.string.host_address_automatic);
        int checked = 0;
        for (int i = 0; i < known.size(); i++) {
            items[i + 1] = known.get(i).toString();
            if (known.get(i).equals(computer.pinnedAddress)) {
                checked = i + 1;
            }
        }
        new NavigationDialogBuilder(activity)
                .setTitle(R.string.host_menu_connection_address)
                .setSingleChoiceItems(items, checked, (d, which) -> {
                    d.dismiss();
                    pinAddress(computer, which == 0 ? null : known.get(which - 1));
                })
                .setNegativeButton(android.R.string.cancel, null)
                .show();
    }

    @VisibleForTesting
    void pinAddress(ComputerDetails computer, ComputerDetails.AddressTuple address) {
        ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
        if (binder == null) {
            showError(R.string.host_menu_connection_address, activity.getString(R.string.error_manager_not_running), null);
            return;
        }
        binder.pinAddress(computer.uuid, address);
        computer.pinnedAddress = address;
        if (address != null) {
            computer.activeAddress = address;
        }
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
