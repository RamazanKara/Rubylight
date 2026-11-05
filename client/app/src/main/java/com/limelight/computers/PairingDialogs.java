package com.limelight.computers;

import android.app.Activity;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.GridLayout;
import android.widget.TextView;
import android.widget.Toast;

import androidx.annotation.VisibleForTesting;
import androidx.appcompat.app.AlertDialog;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePairing;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.NavigationDialogBuilder;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.Executor;

/** Approving a device from this one: the waiting list, and the PIN that device shows, typed on a digit pad. */
final class PairingDialogs {
    private static final int PIN_LENGTH = 4;

    /** A failure, as HostActions shows them. */
    interface Errors {
        void show(CharSequence title, CharSequence message);
    }

    interface ClientOf {
        SleepFlow.Client of(PairingWatch.Waiting waiting);
    }

    private final Activity activity;
    private final Errors errors;
    private final Executor worker;
    private final Executor main;
    private final Runnable changed;

    PairingDialogs(Activity activity, Errors errors, Executor worker, Executor main, Runnable changed) {
        this.activity = activity;
        this.errors = errors;
        this.worker = worker;
        this.main = main;
        this.changed = changed;
    }

    private boolean gone() {
        return activity.isFinishing() || activity.isDestroyed();
    }

    /** The menu's Pair another device: what {@code host} has waiting, read fresh. */
    void showRequests(ComputerDetails host, SleepFlow.Client client) {
        worker.execute(() -> {
            try {
                List<PairingWatch.Waiting> waiting = new ArrayList<>();
                for (NativePairing.Request request : client.open().pairingRequests()) {
                    waiting.add(new PairingWatch.Waiting(host, request));
                }
                main.execute(() -> {
                    if (waiting.isEmpty()) {
                        showNoneWaiting(host);
                    } else if (waiting.size() == 1) {
                        showPin(waiting.get(0), client);
                    } else {
                        showList(waiting, entry -> client);
                    }
                });
            } catch (NativeApiException e) {
                main.execute(() -> errors.show(activity.getString(R.string.host_menu_pair_another), e.getMessage()));
            }
        });
    }

    private void showNoneWaiting(ComputerDetails host) {
        if (gone()) {
            return;
        }
        new NavigationDialogBuilder(activity).setTitle(R.string.host_menu_pair_another)
                .setMessage(activity.getString(R.string.pairing_none_waiting, host.displayName()))
                .setPositiveButton(android.R.string.ok, null)
                .show();
    }

    /** More than one device waits. The address stands beside the name, which is the device's to choose. */
    void showList(List<PairingWatch.Waiting> waiting, ClientOf clients) {
        if (gone()) {
            return;
        }
        CharSequence[] rows = new CharSequence[waiting.size()];
        for (int i = 0; i < rows.length; i++) {
            NativePairing.Request request = waiting.get(i).request;
            rows[i] = activity.getString(R.string.pairing_request_row, request.name, request.address,
                    waiting.get(i).host.displayName());
        }
        new NavigationDialogBuilder(activity).setTitle(R.string.host_menu_pair_another)
                .setItems(rows, (dialog, which) -> showPin(waiting.get(which), clients.of(waiting.get(which))))
                .setNegativeButton(android.R.string.cancel, null)
                .show();
    }

    /** Approve stays off until the PIN is whole, and takes the focus then. Back leaves the request waiting. */
    @VisibleForTesting
    AlertDialog showPin(final PairingWatch.Waiting waiting, final SleepFlow.Client client) {
        if (gone()) {
            return null;
        }
        final NativePairing.Request request = waiting.request;
        View content = activity.getLayoutInflater().inflate(R.layout.dialog_pairing_pin, null);
        ((TextView) content.findViewById(R.id.pairing_pin_from)).setText(
                activity.getString(R.string.pairing_pin_from, request.address, waiting.host.displayName()));
        final TextView value = content.findViewById(R.id.pairing_pin_value);
        final GridLayout pad = content.findViewById(R.id.pairing_pin_pad);
        final StringBuilder pin = new StringBuilder();

        final AlertDialog dialog = new NavigationDialogBuilder(activity)
                .setTitle(activity.getString(R.string.pairing_wants_to_pair, request.name))
                .setView(content)
                .setNegativeButton(R.string.pairing_deny, null)
                .setPositiveButton(R.string.pairing_approve, null)
                .setOnKeyListener((shown, key, event) -> {
                    int digit = digitOf(key);
                    boolean erase = key == KeyEvent.KEYCODE_DEL;
                    if (digit < 0 && !erase) {
                        return false;
                    }
                    if (event.getAction() == KeyEvent.ACTION_DOWN) {
                        if (erase) {
                            eraseDigit((AlertDialog) shown, pad, pin, value);
                        } else {
                            typeDigit((AlertDialog) shown, pad, pin, value, digit);
                        }
                    }
                    return true;
                })
                .create();

        for (int digit = 1; digit <= 9; digit++) {
            addDigit(dialog, pad, pin, value, digit);
        }
        MaterialButton erase = padButton(pad);
        erase.setIconResource(R.drawable.ic_backspace);
        erase.setIconPadding(0);
        erase.setContentDescription(activity.getString(R.string.pairing_pin_erase));
        erase.setOnClickListener(v -> eraseDigit(dialog, pad, pin, value));
        addDigit(dialog, pad, pin, value, 0);

        dialog.setOnShowListener(shown -> {
            View approve = dialog.getButton(AlertDialog.BUTTON_POSITIVE);
            approve.setEnabled(false);
            approve.setOnClickListener(v -> approve(dialog, pad, waiting, client, pin.toString(), value));
            dialog.getButton(AlertDialog.BUTTON_NEGATIVE).setOnClickListener(v -> deny(dialog, pad, waiting, client));
            pad.getChildAt(0).requestFocus();
        });
        dialog.show();
        return dialog;
    }

    private static int digitOf(int key) {
        if (key >= KeyEvent.KEYCODE_0 && key <= KeyEvent.KEYCODE_9) {
            return key - KeyEvent.KEYCODE_0;
        }
        if (key >= KeyEvent.KEYCODE_NUMPAD_0 && key <= KeyEvent.KEYCODE_NUMPAD_9) {
            return key - KeyEvent.KEYCODE_NUMPAD_0;
        }
        return -1;
    }

    private MaterialButton padButton(GridLayout pad) {
        MaterialButton button = new MaterialButton(pad.getContext(), null, R.attr.nightjarButtonTonalStyle);
        GridLayout.LayoutParams params = new GridLayout.LayoutParams();
        int margin = activity.getResources().getDimensionPixelSize(R.dimen.space_xs);
        params.setMargins(margin, margin, margin, margin);
        params.width = activity.getResources().getDimensionPixelSize(R.dimen.touch_target) * 2;
        button.setLayoutParams(params);
        pad.addView(button);
        return button;
    }

    private void addDigit(AlertDialog dialog, GridLayout pad, StringBuilder pin, TextView value, int digit) {
        MaterialButton button = padButton(pad);
        button.setText(String.valueOf(digit));
        button.setOnClickListener(v -> typeDigit(dialog, pad, pin, value, digit));
    }

    private void typeDigit(AlertDialog dialog, ViewGroup pad, StringBuilder pin, TextView value, int digit) {
        // A pad that is off means the host is answering; keys reach this too.
        if (pin.length() >= PIN_LENGTH || !pad.getChildAt(0).isEnabled()) {
            return;
        }
        pin.append(digit);
        value.setText(pin);
        if (pin.length() == PIN_LENGTH) {
            View approve = dialog.getButton(AlertDialog.BUTTON_POSITIVE);
            approve.setEnabled(true);
            approve.requestFocus();
        }
    }

    private void eraseDigit(AlertDialog dialog, ViewGroup pad, StringBuilder pin, TextView value) {
        if (pin.length() == 0 || !pad.getChildAt(0).isEnabled()) {
            return;
        }
        pin.setLength(pin.length() - 1);
        value.setText(pin);
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).setEnabled(false);
    }

    /** Buttons and pad wait, and Back with them: the host is finishing the handshake. */
    private static void setBusy(AlertDialog dialog, ViewGroup pad) {
        dialog.setCancelable(false);
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).setEnabled(false);
        dialog.getButton(AlertDialog.BUTTON_NEGATIVE).setEnabled(false);
        for (int i = 0; i < pad.getChildCount(); i++) {
            pad.getChildAt(i).setEnabled(false);
        }
    }

    private void approve(AlertDialog dialog, ViewGroup pad, PairingWatch.Waiting waiting, SleepFlow.Client client,
                         String pin, TextView value) {
        final String name = waiting.request.name;
        setBusy(dialog, pad);
        value.setText(activity.getString(R.string.pairing_waiting_for, name));
        worker.execute(() -> {
            NativeApiException failure = null;
            String paired = null;
            try {
                paired = client.open().approvePairing(waiting.request.id, pin);
            } catch (NativeApiException e) {
                failure = e;
            }
            final NativeApiException failed = failure;
            final String approved = paired;
            main.execute(() -> {
                finish(dialog);
                if (failed != null) {
                    errors.show(activity.getString(R.string.pairing_wants_to_pair, name), failed.getMessage());
                } else {
                    Toast.makeText(activity.getApplicationContext(),
                            activity.getString(R.string.pairing_approved, approved), Toast.LENGTH_LONG).show();
                }
                changed.run();
            });
        });
    }

    private void deny(AlertDialog dialog, ViewGroup pad, PairingWatch.Waiting waiting, SleepFlow.Client client) {
        setBusy(dialog, pad);
        worker.execute(() -> {
            NativeApiException failure = null;
            try {
                client.open().denyPairing(waiting.request.id);
            } catch (NativeApiException e) {
                failure = e;
            }
            final NativeApiException failed = failure;
            main.execute(() -> {
                finish(dialog);
                if (failed != null) {
                    errors.show(activity.getString(R.string.pairing_wants_to_pair, waiting.request.name),
                            failed.getMessage());
                }
                changed.run();
            });
        });
    }

    private void finish(AlertDialog dialog) {
        if (!gone()) {
            dialog.dismiss();
        }
    }
}
