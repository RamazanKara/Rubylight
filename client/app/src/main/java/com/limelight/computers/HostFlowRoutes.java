package com.limelight.computers;

import android.app.Activity;
import android.content.Context;
import android.os.Build;
import android.text.InputFilter;
import android.text.InputType;
import android.util.Base64;
import android.view.View;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.widget.EditText;
import android.widget.Toast;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.PlatformBinding;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.KnownUpdates;
import com.limelight.nativeapi.HostErrorText;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePcPower;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingLink;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.nvstream.jni.NightjarHostsJni;
import com.limelight.nvstream.jni.NightjarHttpJni;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;

import org.json.JSONException;
import org.json.JSONObject;

import androidx.appcompat.app.AlertDialog;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.security.cert.CertificateEncodingException;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.List;
import java.util.function.BiConsumer;
import java.util.function.Consumer;

/**
 * UI-thread routing and localized string translation for Sleep PC, Restart/Shut down PC,
 * Host Update, Pairing, and Unpair flows backed by Rust {@code nightjar_core::hosts}.
 */
final class HostFlowRoutes {
    private HostFlowRoutes() {
    }

    static boolean refused(int status) {
        return NightjarPolicyJni.pairingRefusedStatus(status);
    }

    static int refusalMessage(boolean fromLink, boolean oneTimePin) {
        int code = NightjarPolicyJni.classifyPairingHttpRefusal(fromLink, oneTimePin);
        if (code == 2) return R.string.pair_qr_incorrect_pin;
        if (code == 1) return R.string.pair_otp_incorrect_pin;
        return R.string.pair_fail;
    }

    static int failureMessage(PairState state, boolean fromLink, boolean oneTimePin, boolean inGame) {
        return pairFailureRes(NightjarPolicyJni.classifyPairingFailure(
                state.ordinal(), fromLink, oneTimePin, inGame));
    }

    private static int pairFailureRes(int kind) {
        switch (kind) {
            case 0: return R.string.pair_incorrect_pin;
            case 1: return R.string.pair_otp_incorrect_pin;
            case 2: return R.string.pair_qr_incorrect_pin;
            case 3: return R.string.pair_already_in_progress;
            case 4: return R.string.pair_unverified;
            case 5: return R.string.pair_pc_ingame;
            case 7: return R.string.pair_qr_cert_mismatch;
            case 8: return R.string.pair_connection_fix;
            default: return R.string.pair_fail;
        }
    }

    static boolean alreadyPaired(PairState state, PairingLink link, X509Certificate pinned) {
        return NightjarPolicyJni.alreadyPaired(state == PairState.PAIRED, link != null, pinned != null);
    }

    static int unpairErrorMessage(int kind) {
        switch (kind) {
            case 0: return R.string.unpair_error;
            case 1: return R.string.error_unknown_host;
            case 2: return R.string.error_404;
            default: return R.string.unpair_fail;
        }
    }

    static int powerTitle(NativePcPower.Action action) {
        return action == NativePcPower.Action.RESTART
                ? R.string.host_menu_restart : R.string.host_menu_shutdown;
    }

    static String sleepUnavailableText(Context context, NativeApiClient.PowerReply offer) {
        switch (NightjarPolicyJni.sleepUnavailableReason(offer.sleepSupported, offer.sleepAllowed)) {
            case 0: return context.getString(R.string.host_sleep_unsupported);
            case 1: return context.getString(R.string.host_feature_reason_sleep_permission) + "\n\n"
                    + context.getString(R.string.host_sleep_permission_help);
            default: return context.getString(R.string.host_sleep_busy);
        }
    }

    static String sleepRefusedText(Context context, String message, int httpStatus) {
        return message.isEmpty() ? context.getString(R.string.host_sleep_refused, httpStatus) : message;
    }

    static boolean sleepRetries(NativeApiException error) {
        return error.getKind() == NativeApiException.Kind.NETWORK
                || error.getKind() == NativeApiException.Kind.TLS;
    }

    static String sleepFailedText(Context context, NativeApiException error) {
        if (sleepRetries(error)) return context.getString(R.string.host_sleep_failed);
        if (error.getKind() == NativeApiException.Kind.NOT_PAIRED) return context.getString(R.string.host_feature_reason_pair);
        return HostErrorText.around(context.getResources(),
                context.getString(R.string.host_sleep_error, error.getMessage()), error);
    }

    static String powerUnavailableText(Context context, String reason) {
        return reason.isEmpty() ? context.getString(R.string.host_power_busy) : reason;
    }

    static String powerConfirmText(Context context, NativePcPower.Action action, boolean wakeFromOff) {
        String text = context.getString(R.string.host_power_message);
        if (action == NativePcPower.Action.SHUT_DOWN && !wakeFromOff) {
            text += "\n\n" + context.getString(R.string.host_shutdown_no_wake);
        }
        return text;
    }

    static String powerAcceptedText(Context context, NativePcPower.Action action, String name) {
        return context.getString(
                action == NativePcPower.Action.RESTART ? R.string.host_restart_accepted : R.string.host_shutdown_accepted,
                name);
    }

    static class Route {
        final Context app;
        private final String what;
        private HostActions screen;
        private boolean dropped;

        Route(Context app, HostActions screen, String what) {
            this.app = app;
            this.screen = screen;
            this.what = what;
        }

        void detach() { screen = null; }
        void drop() { dropped = true; }
        HostActions screen() { return dropped ? null : screen; }

        void deliver(String outcome, Consumer<HostActions> shown, String toast) {
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

    static SleepFlow.Ui sleepUi(final ComputerDetails computer, final Route route) {
        final Context app = route.app;
        final String title = app.getString(R.string.host_menu_sleep);
        return new SleepFlow.Ui() {
            @Override public void confirm(Runnable sleep) {
                route.deliver("offer", screen -> {
                    screen.dismissSleepProgress();
                    screen.showSleepConfirm(computer, sleep);
                }, null);
            }
            @Override public void accepted() {
                String text = app.getString(R.string.host_sleep_accepted);
                route.deliver("acceptance", screen -> {
                    screen.dismissSleepProgress();
                    if (!screen.gone()) Toast.makeText(screen.activity(), text, Toast.LENGTH_LONG).show();
                }, text);
            }
            @Override public void unavailable(NativeApiClient.PowerReply offer) {
                String text = sleepUnavailableText(app, offer);
                boolean busy = offer.sleepSupported && offer.sleepAllowed;
                route.deliver("answer", screen -> {
                    screen.dismissSleepProgress();
                    screen.showError(title, text, busy ? () -> screen.sleep(computer) : null);
                }, text);
            }
            @Override public void refused(String message, int httpStatus) {
                String text = sleepRefusedText(app, message, httpStatus);
                route.deliver("refusal", screen -> {
                    screen.dismissSleepProgress();
                    screen.showError(title, text, httpStatus == 409 ? () -> screen.sleep(computer) : null);
                }, text);
            }
            @Override public void failed(NativeApiException error) {
                String text = sleepFailedText(app, error);
                route.deliver("failure", screen -> {
                    screen.dismissSleepProgress();
                    screen.showError(title, text, sleepRetries(error) ? () -> screen.sleep(computer) : null);
                }, text);
            }
        };
    }

    static PcPowerFlow.Ui pcPowerUi(
            final ComputerDetails computer, final NativePcPower.Action action, final Route route) {
        final Context app = route.app;
        final String title = app.getString(powerTitle(action));
        final String name = computer.displayName();
        return new PcPowerFlow.Ui() {
            @Override public void confirm(boolean wakeFromOff, Runnable go) {
                route.deliver("offer", screen -> {
                    screen.dismissPowerProgress();
                    screen.confirmWithCancelFocused(
                            app.getString(action == NativePcPower.Action.RESTART
                                    ? R.string.host_restart_title : R.string.host_shutdown_title, name),
                            powerConfirmText(app, action, wakeFromOff),
                            action == NativePcPower.Action.RESTART
                                    ? R.string.host_restart_action : R.string.host_shutdown_action,
                            () -> { screen.showPowerProgress(powerTitle(action), true); go.run(); });
                }, null);
            }
            @Override public void accepted() {
                String text = powerAcceptedText(app, action, name);
                route.deliver("acceptance", screen -> {
                    screen.dismissPowerProgress();
                    if (!screen.gone()) Toast.makeText(screen.activity(), text, Toast.LENGTH_LONG).show();
                }, text);
            }
            @Override public void unavailable(String reason) {
                String text = powerUnavailableText(app, reason);
                route.deliver("answer: " + reason, screen -> {
                    screen.dismissPowerProgress();
                    screen.showError(title, text, null);
                }, text);
            }
            @Override public void refused(String message) {
                route.deliver("refusal: " + message, screen -> {
                    screen.dismissPowerProgress();
                    screen.showError(title, message, null);
                }, message);
            }
            @Override public void failed(NativeApiException error) {
                String text = sleepFailedText(app, error);
                route.deliver("failure: " + error.getKind() + " " + error.getMessage(), screen -> {
                    screen.dismissPowerProgress();
                    screen.showError(title, text, sleepRetries(error) ? () -> screen.pcPower(computer, action) : null);
                }, text);
            }
            @Override public void refusedLater(String refusal, boolean unconfirmed) {
                boolean restart = action == NativePcPower.Action.RESTART;
                String laterTitle = app.getString(unconfirmed
                        ? (restart ? R.string.host_restart_unconfirmed_title : R.string.host_shutdown_unconfirmed_title)
                        : (restart ? R.string.host_restart_refused_title : R.string.host_shutdown_refused_title), name);
                route.deliver((unconfirmed ? "unconfirmed: " : "refusal later: ") + refusal, screen -> {
                    screen.dismissPowerProgress();
                    screen.showError(laterTitle, refusal, null);
                }, laterTitle + ": " + refusal);
            }
        };
    }

    static HostUpdateFlow.Ui updateUi(final ComputerDetails computer, final Route route) {
        final Context app = route.app;
        final String name = computer.displayName();
        final String uuid = computer.uuid;
        final String failedTitle = app.getString(R.string.host_update_failed_title, name);
        return new HostUpdateFlow.Ui() {
            @Override public void started(String version) {
                route.deliver("start " + version, screen -> screen.showUpdateDialog(
                        app.getString(R.string.host_update_started_title, name),
                        app.getString(R.string.host_update_started, version)), null);
            }
            @Override public void waiting(String reason) {
                route.deliver("waiting: " + reason, screen -> screen.showUpdateDialog(
                        app.getString(R.string.host_update_started_title, name),
                        app.getString(R.string.host_update_waiting, reason)), null);
            }
            @Override public void back(String version) {
                KnownUpdates.forget(uuid);
                String title = app.getString(R.string.host_update_back_title, name);
                String text = app.getString(R.string.host_update_back, version);
                route.deliver("back on " + version, screen -> {
                    screen.dismissUpdateDialog();
                    screen.showError(title, text, null);
                }, title);
            }
            @Override public void timedOut(String version) {
                endUpdateAndShow(route, failedTitle, app.getString(R.string.host_update_timeout, version), "timeout");
            }
            @Override public void nothingToInstall() {
                endUpdateAndShow(route, failedTitle, app.getString(R.string.host_update_none), "nothing to install");
            }
            @Override public void updateFailed(String error) {
                endUpdateAndShow(route, failedTitle, error != null ? error : app.getString(R.string.host_update_failed),
                        "failed on the PC: " + error);
            }
            @Override public void cameBackOn(String version, String error) {
                String back = app.getString(R.string.host_update_old_version, version);
                endUpdateAndShow(route, failedTitle, error != null ? back + " " + error : back, "back on old " + version);
            }
            @Override public void refused(String message) {
                endUpdateAndShow(route, failedTitle, message, "refusal: " + message);
            }
            @Override public void failed(NativeApiException error) {
                endUpdateAndShow(route, failedTitle, sleepFailedText(app, error),
                        "failure: " + error.getKind() + " " + error.getMessage());
            }
        };
    }

    private static void endUpdateAndShow(Route route, String title, String text, String outcome) {
        route.deliver(outcome, screen -> {
            screen.dismissUpdateDialog();
            screen.showError(title, text, null);
        }, text);
    }

    static AlertDialog showPairOneTimePinDialog(
            Activity activity, ComputerDetails computer, BiConsumer<String, String> onSubmit) {
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
                .setTitle(R.string.pair_otp_title).setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.pcview_menu_pair_pc, null).create();
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
            onSubmit.accept(pinValue, passphraseValue);
        }));
        dialog.setOnDismissListener(ignored -> { pin.getText().clear(); passphrase.getText().clear(); });
        dialog.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        dialog.show();
        return dialog;
    }

    static AlertDialog showRenameDialog(Activity activity, ComputerDetails computer, Consumer<String> onSave) {
        View fields = activity.getLayoutInflater().inflate(R.layout.dialog_rename_host, null);
        EditText name = fields.findViewById(R.id.rename_name);
        name.setFilters(new InputFilter[] {new InputFilter.LengthFilter(HostNames.MAX_LENGTH)});
        ((com.google.android.material.textfield.TextInputLayout) fields.findViewById(R.id.rename_field))
                .setPlaceholderText(computer.name);
        name.setSaveEnabled(false);
        name.setText(computer.displayName());
        name.setSelection(name.getText().length());
        AlertDialog dialog = new NavigationDialogBuilder(activity)
                .setTitle(R.string.host_rename_title).setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.host_rename_save, (d, w) -> onSave.accept(name.getText().toString()))
                .create();
        name.setOnEditorActionListener((view, action, event) -> {
            if (action != EditorInfo.IME_ACTION_DONE) return false;
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            return true;
        });
        dialog.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE
                | WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE);
        dialog.show();
        return dialog;
    }

    static AlertDialog showChooseAddressDialog(
            Activity activity, ComputerDetails computer, Consumer<ComputerDetails.AddressTuple> onSelect) {
        List<ComputerDetails.AddressTuple> known = computer.knownAddresses();
        if (computer.pinnedAddress != null && !known.contains(computer.pinnedAddress)) known.add(computer.pinnedAddress);
        String[] items = new String[known.size() + 1];
        items[0] = activity.getString(R.string.host_address_automatic);
        int checked = 0;
        for (int i = 0; i < known.size(); i++) {
            items[i + 1] = known.get(i).toString();
            if (known.get(i).equals(computer.pinnedAddress)) checked = i + 1;
        }
        return new NavigationDialogBuilder(activity)
                .setTitle(R.string.host_menu_connection_address)
                .setSingleChoiceItems(items, checked, (d, which) -> {
                    d.dismiss();
                    onSelect.accept(which == 0 ? null : known.get(which - 1));
                })
                .setNegativeButton(android.R.string.cancel, null).show();
    }

    private static byte[] encodeCert(X509Certificate cert) throws CertificateEncodingException {
        return cert == null ? new byte[0] : cert.getEncoded();
    }

    static void runUnpairWorker(
            Activity activity, HostActions actions, HostActions.Callbacks callbacks,
            ComputerManagerService.ComputerManagerBinder binder, ComputerDetails computer) {
        String failure = null;
        boolean retry = true;
        boolean unpaired = false;
        try {
            LimelightCryptoProvider crypto = PlatformBinding.getCryptoProvider(activity);
            byte[] clientCert = crypto == null ? new byte[0] : encodeCert(crypto.getClientCertificate());
            byte[] clientKey = crypto == null || crypto.getClientPrivateKey() == null
                    ? new byte[0] : crypto.getClientPrivateKey().getEncoded();
            String deviceName = Build.MODEL != null && !Build.MODEL.isEmpty() ? Build.MODEL : "Nightjar";
            ComputerDetails.AddressTuple addr = ServerHelper.getCurrentAddressFromComputer(computer);
            int res = addr == null ? 3 : NightjarHostsJni.hostFlowUnpair(
                    addr.address, addr.port, computer.httpsPort, binder.getUniqueId(),
                    clientCert, clientKey, encodeCert(computer.serverCert), deviceName);
            if (res == -1) {
                unpaired = true;
                binder.forgetCertificate(computer.uuid);
                binder.invalidateStateForComputer(computer.uuid);
            } else {
                failure = activity.getString(unpairErrorMessage(res));
                if (res == 0) retry = false;
            }
        } catch (CertificateEncodingException | IOException e) {
            LimeLog.warning("Unpair failed: " + e);
            failure = activity.getString(unpairErrorMessage(3));
        }
        final boolean done = unpaired;
        final String message = failure;
        final boolean canRetry = retry;
        activity.runOnUiThread(() -> {
            if (done) {
                ConsoleRepository.forgetHost(activity, computer.uuid);
                if (!actions.gone()) Toast.makeText(activity, R.string.unpair_success, Toast.LENGTH_LONG).show();
            } else if (callbacks.isForeground() && !actions.gone()) {
                actions.showError(R.string.pcview_menu_unpair_pc, message, canRetry ? () -> actions.unpair(computer) : null);
            } else {
                Toast.makeText(activity, message, Toast.LENGTH_LONG).show();
            }
        });
    }

    static AlertDialog showPairingDialog(
            Activity activity, String oneTimePin, String pin, Runnable onCancel) {
        android.text.SpannableStringBuilder pairingText = new android.text.SpannableStringBuilder();
        if (oneTimePin == null) {
            pairingText.append(activity.getString(R.string.pair_pairing_help)).append("\n\n");
            int pinStart = pairingText.length();
            pairingText.append(pin);
            pairingText.setSpan(new android.text.style.RelativeSizeSpan(2.25f), pinStart, pairingText.length(), 0);
            pairingText.setSpan(new android.text.style.StyleSpan(android.graphics.Typeface.BOLD), pinStart, pairingText.length(), 0);
            pairingText.setSpan(new android.text.style.ForegroundColorSpan(com.google.android.material.color.MaterialColors
                    .getColor(activity, androidx.appcompat.R.attr.colorPrimary, android.graphics.Color.WHITE)),
                    pinStart, pairingText.length(), 0);
        } else {
            pairingText.append(activity.getString(R.string.pairing));
        }
        AlertDialog dialog = new NavigationDialogBuilder(activity).ownSurface()
                .setTitle(R.string.pair_pairing_title).setMessage(pairingText)
                .setNegativeButton(android.R.string.cancel, (d, which) -> onCancel.run())
                .setOnCancelListener(d -> onCancel.run()).create();
        dialog.show();
        return dialog;
    }

    static void runPairWorker(
            Activity activity, HostActions actions, ComputerManagerService.ComputerManagerBinder binder,
            ComputerDetails computer, PairingLink link, String pin, String oneTimePin,
            String passphrase, int generation) {
        String message = null;
        boolean success = false;
        long httpHandle = 0;
        try {
            binder.waitForPollingStopped();
            ComputerDetails.AddressTuple addr = ServerHelper.getCurrentAddressFromComputer(computer);
            if (addr == null) {
                message = activity.getString(R.string.pair_connection_fix);
            } else {
                LimelightCryptoProvider crypto = PlatformBinding.getCryptoProvider(activity.getApplicationContext());
                byte[] clientCert = crypto == null ? new byte[0] : encodeCert(crypto.getClientCertificate());
                byte[] clientKey = crypto == null || crypto.getClientPrivateKey() == null
                        ? new byte[0] : crypto.getClientPrivateKey().getEncoded();
                String deviceName = Build.MODEL != null && !Build.MODEL.isEmpty() ? Build.MODEL : "Nightjar";
                httpHandle = NightjarHttpJni.clientCreate(addr.address, addr.port, computer.httpsPort,
                        binder.getUniqueId(), clientCert, clientKey, encodeCert(computer.serverCert), deviceName);
                if (httpHandle <= 0) {
                    message = activity.getString(R.string.pair_connection_fix);
                } else if (!actions.acceptPairingRequest(generation, Long.valueOf(httpHandle))) {
                    NightjarHttpJni.clientCancelPendingRequests(httpHandle);
                    return;
                } else {
                    int mode = link != null ? 2 : (oneTimePin != null ? 1 : 0);
                    JSONObject res = new JSONObject(NightjarHostsJni.hostFlowPair(
                            httpHandle, binder.hostManagerHandle(), mode, pin,
                            passphrase == null ? "" : passphrase, link == null ? "" : link.text,
                            computer.serverCert != null, computer.runningGameId != 0));
                    if (res.optBoolean("ok")) {
                        byte[] der = Base64.decode(res.optString("cert_b64", ""), Base64.DEFAULT);
                        ComputerDetails stored = binder.getComputer(computer.uuid);
                        if (stored != null && der.length > 0) {
                            stored.serverCert = (X509Certificate) CertificateFactory.getInstance("X.509")
                                    .generateCertificate(new ByteArrayInputStream(der));
                            binder.setServerCertificate(computer.uuid, der);
                            binder.invalidateStateForComputer(computer.uuid);
                            success = true;
                            com.limelight.console.SetupCard.offer(activity.getApplicationContext(), stored);
                        } else {
                            message = activity.getString(R.string.pair_fail);
                        }
                    } else {
                        message = activity.getString(pairFailureRes(res.optInt("kind", 10)));
                    }
                }
            }
        } catch (CertificateException | IOException | JSONException e) {
            LimeLog.warning("Pairing failed: " + e.getMessage());
            message = activity.getString(R.string.pair_connection_fix);
        } finally {
            if (httpHandle > 0) NightjarHttpJni.clientDestroy(httpHandle);
        }
        final boolean paired = success;
        final String failure = message;
        activity.runOnUiThread(() -> actions.showPairingResult(generation, paired, failure, computer, oneTimePin, link != null));
    }

    static CoopSheet.Ui coopUi(
            Activity activity, HostActions actions, HostActions.Callbacks callbacks, ComputerDetails computer) {
        return new CoopSheet.Ui() {
            @Override public void problem(CharSequence title, CharSequence message, Runnable retry) {
                actions.showError(title, message, retry);
            }
            @Override public void start(NvApp app) {
                ComputerManagerService.ComputerManagerBinder binder = callbacks.binder();
                if (binder == null) {
                    actions.showError(R.string.host_menu_coop, activity.getString(R.string.error_manager_not_running), null);
                    return;
                }
                ServerHelper.doStart(activity, app, computer, binder, LaunchSource.COOP_JOIN);
            }
            @Override public void reopen() { actions.joinCoop(computer); }
        };
    }
}
