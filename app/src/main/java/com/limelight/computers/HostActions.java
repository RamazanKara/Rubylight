package com.limelight.computers;

import android.app.Activity;
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
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.http.PairingManager.PairState;
import com.limelight.nvstream.wol.WakeOnLanSender;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;

import org.xmlpull.v1.XmlPullParserException;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.net.UnknownHostException;

/**
 * What a person can do to a PC on the list besides opening it: pair (with a PIN shown here or a
 * one-time PIN typed here), wake, rename, unpair and remove. It belongs to one screen and its binder.
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

    /** The screen is going away: nothing a pairing finds out later may reach it. */
    public void destroy() {
        cancelPairing();
        pendingPairingResult = null;
    }

    // Pairing

    public void pair(ComputerDetails computer) {
        pair(computer, null, null);
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
            pair(computer, pinValue, passphraseValue);
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

    private void pair(final ComputerDetails computer, final String oneTimePin, final String passphrase) {
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
                if (request.getPairState() == PairState.PAIRED) {
                    success = true;
                } else {
                    PairingManager pairing = request.getPairingManager();
                    PairState state = pairing.pair(request.getServerInfo(true), pin, passphrase);
                    if (state == PairState.PAIRED) {
                        ComputerDetails stored = binder.getComputer(computer.uuid);
                        if (stored != null) {
                            stored.serverCert = pairing.getPairedCert();
                            binder.invalidateStateForComputer(computer.uuid);
                            success = true;
                        } else {
                            message = activity.getString(R.string.pair_fail);
                        }
                    } else {
                        message = activity.getString(state == PairState.PIN_WRONG ?
                                (oneTimePin == null ? R.string.pair_incorrect_pin : R.string.pair_otp_incorrect_pin) :
                                state == PairState.ALREADY_IN_PROGRESS ? R.string.pair_already_in_progress :
                                        computer.runningGameId != 0 ? R.string.pair_pc_ingame : R.string.pair_fail);
                    }
                }
            } catch (IOException | XmlPullParserException e) {
                message = activity.getString(R.string.pair_connection_fix);
            }
            final boolean paired = success;
            final String failure = message;
            activity.runOnUiThread(() -> showPairingResult(generation, paired, failure, computer, oneTimePin));
        }, "Pairing");
        pairingThread.start();
    }

    @VisibleForTesting
    void showPairingResult(int generation, boolean paired, String failure,
                           ComputerDetails computer, String oneTimePin) {
        if (generation != pairingGeneration || gone()) {
            return;
        }
        if (!callbacks.isForeground()) {
            pendingPairingResult = () -> showPairingResult(generation, paired, failure, computer, oneTimePin);
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
            showError(R.string.pair_pairing_title, failure, () -> {
                if (oneTimePin == null) {
                    pair(computer);
                } else {
                    pairWithOneTimePin(computer);
                }
            });
        }
    }

    // Wake

    public void wake(final ComputerDetails computer) {
        if (computer.state == ComputerDetails.State.ONLINE) {
            Toast.makeText(activity, R.string.wol_pc_online, Toast.LENGTH_SHORT).show();
            return;
        }
        if (computer.macAddress == null) {
            showError(R.string.pcview_menu_send_wol, activity.getString(R.string.wol_no_mac), null);
            return;
        }
        new Thread(() -> {
            boolean sent;
            try {
                WakeOnLanSender.sendWolPacket(computer);
                sent = true;
            } catch (IOException e) {
                sent = false;
            }
            final boolean woken = sent;
            activity.runOnUiThread(() -> {
                if (gone()) return;
                if (woken) {
                    callbacks.onWakeSent(computer);
                    Toast.makeText(activity, R.string.wol_waking_msg, Toast.LENGTH_LONG).show();
                } else {
                    showError(R.string.pcview_menu_send_wol, activity.getString(R.string.wol_fail), () -> wake(computer));
                }
            });
        }, "Wake on LAN").start();
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
                    Toast.makeText(activity, R.string.unpair_success, Toast.LENGTH_LONG).show();
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
