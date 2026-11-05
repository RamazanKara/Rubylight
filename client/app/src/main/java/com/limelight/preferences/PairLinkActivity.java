package com.limelight.preferences;

import android.content.Intent;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.View;
import android.widget.Button;

import androidx.annotation.NonNull;
import androidx.appcompat.app.AlertDialog;

import com.limelight.PcView;
import com.limelight.R;
import com.limelight.nvstream.http.PairingLink;
import com.limelight.pin.PinGate;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.NightjarActivity;
import com.limelight.utils.UiHelper;

/**
 * Takes a rubylight://pair link from a browser, a QR reader or any other app. Any web page can open one, so nothing
 * is added or paired until the user says so; the scanner and the address field in {@link AddComputerManually} go
 * straight to pairing.
 *
 * <p>This screen is a window with only the question in it, in a task of its own that leaves no trace in Recents.
 * While the gate is held ({@link LinkGate}) it ends before it is ever shown: a stream under it does not stop, because
 * only {@code Game.onStop} stops one and a screen that ends in onCreate never covers it.
 */
public class PairLinkActivity extends NightjarActivity {
    /** How long Pair stays off after a question appears: a tap meant for what was there before must not land on it. */
    static final long PAIR_DELAY_MS = 1000;

    private final Handler main = new Handler(Looper.getMainLooper());
    private AlertDialog question;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        // AppTheme.apply has just replaced the theme the manifest names.
        getTheme().applyStyle(R.style.ThemeOverlay_Nightjar_PairLink, true);
        UiHelper.setLocale(this);
        receive(getIntent());
        askNext();
    }

    @Override
    protected void onNewIntent(@NonNull Intent intent) {
        super.onNewIntent(intent);
        receive(intent);
        // A question already up stays as it is, whatever comes in: no link can swap it under a finger.
        if (question == null) {
            askNext();
        }
    }

    /** Files the link and drops the intent that carries its PIN and passphrase. */
    private void receive(Intent intent) {
        if (Intent.ACTION_VIEW.equals(intent.getAction()) && intent.getData() != null
                && (intent.getFlags() & Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) == 0) {
            LinkGate.put(intent.getDataString());
        }
        setIntent(new Intent());
    }

    private void askNext() {
        question = null;
        main.removeCallbacksAndMessages(null);
        if (!LinkGate.hasWaiting() || LinkGate.held()) {
            finish();
        } else if (PinGate.needsPinForApp(this)) {
            // This screen would be gone behind the PIN screen; the link is asked about after it.
            startActivity(new Intent(this, PcView.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            finish();
        } else {
            PairingLink link = PairingLink.parse(LinkGate.take());
            if (link == null) {
                refuse();
            } else {
                confirm(link);
            }
        }
    }

    private void refuse() {
        question = new NavigationDialogBuilder(this)
                .setTitle(R.string.conn_error_title)
                .setMessage(R.string.addpc_qr_invalid)
                .setPositiveButton(android.R.string.ok, (ignored, which) -> askNext())
                .setOnCancelListener(ignored -> askNext())
                .create();
        question.show();
    }

    private void confirm(PairingLink link) {
        String name = link.shownName();
        String address = getString(R.string.pair_link_address, link.shownAddress());
        AlertDialog dialog = new NavigationDialogBuilder(this)
                .setTitle(R.string.pair_pairing_title)
                .setMessage(name.isEmpty() ? address : name + "\n" + address)
                .setNegativeButton(android.R.string.cancel, (ignored, which) -> askNext())
                .setPositiveButton(R.string.pcview_menu_pair_pc, (ignored, which) -> pair(link))
                .setOnCancelListener(ignored -> askNext())
                .create();
        // Cancel has focus: a key or button already held down must not pair. An overlay cannot tap Pair for the user.
        dialog.setOnShowListener(ignored -> {
            Button pair = dialog.getButton(AlertDialog.BUTTON_POSITIVE);
            pair.setFilterTouchesWhenObscured(true);
            pair.setEnabled(false);
            main.postDelayed(() -> pair.setEnabled(true), PAIR_DELAY_MS);
            View cancel = dialog.getButton(AlertDialog.BUTTON_NEGATIVE);
            cancel.setFocusable(true);
            cancel.setFocusableInTouchMode(true);
            cancel.requestFocus();
        });
        question = dialog;
        dialog.show();
    }

    private void pair(PairingLink link) {
        question = null;
        LinkGate.confirm(link.text);
        startActivity(new Intent(this, AddComputerManually.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .putExtra(AddComputerManually.EXTRA_FROM_LINK, true));
        finish();
    }

    @Override
    protected void onDestroy() {
        main.removeCallbacksAndMessages(null);
        if (question != null) {
            question.dismiss();
            question = null;
        }
        super.onDestroy();
    }
}
