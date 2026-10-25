package com.limelight.pin;

import android.content.Context;
import android.content.Intent;
import android.content.res.Configuration;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.View;
import android.widget.LinearLayout;
import android.widget.TextView;
import android.widget.Toast;

import androidx.activity.OnBackPressedCallback;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.color.MaterialColors;
import com.limelight.AppView;
import com.limelight.Game;
import com.limelight.R;
import com.limelight.ShortcutTrampoline;
import com.limelight.ui.DpadEchoFilter;
import com.limelight.ui.NightjarActivity;
import com.limelight.utils.UiHelper;

import java.util.Arrays;
import java.util.Locale;
import java.util.concurrent.Executor;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * The PIN pad. One screen for asking (opening the app, starting a stream), for checking the current PIN before a
 * change, and for choosing a new one. Keys: touch, a remote or game pad typing a row at a time (see
 * {@link PinKeys}), and a keyboard's number keys. The digits live only in this object's arrays; none reach the
 * saved state, a log or a string.
 */
public class PinLockActivity extends NightjarActivity {
    public enum Purpose { APP, STREAM, TRAMPOLINE, VERIFY, SET }

    static final String EXTRA_PURPOSE = "pin_purpose";
    /** The stream (or shortcut) start that waits for the PIN. */
    public static final String EXTRA_NEXT = "pin_next";
    static final String EXTRA_PC_NAME = "pin_pc_name";
    static final String EXTRA_PC_UUID = "pin_pc_uuid";

    /** Tests run the key derivation on the calling thread. */
    static Executor executorForTests;

    private static Intent base(Context context, Purpose purpose) {
        return new Intent(context, PinLockActivity.class).putExtra(EXTRA_PURPOSE, purpose.name());
    }

    /** Covers the app until the PIN is entered; Back closes the app. */
    public static Intent forApp(Context context) {
        return base(context, Purpose.APP).addFlags(Intent.FLAG_ACTIVITY_NO_ANIMATION);
    }

    /** Asks before {@code next}, the stream's start, runs. */
    public static Intent forStream(Context context, Intent next, String pcName) {
        return base(context, Purpose.STREAM).putExtra(EXTRA_NEXT, next).putExtra(EXTRA_PC_NAME, pcName);
    }

    /**
     * Asks before a shortcut or frontend start runs again. Only the fields the shortcut screen reads are carried
     * over, and the target is always the shortcut screen: the incoming intent comes from another app.
     */
    public static Intent forTrampoline(Context context, Intent incoming, String uuid) {
        Intent again = new Intent(context, ShortcutTrampoline.class);
        for (String key : new String[] {AppView.UUID_EXTRA, AppView.NAME_EXTRA, Game.EXTRA_APP_NAME,
                Game.EXTRA_APP_ID, Game.EXTRA_APP_UUID}) {
            String value = incoming.getStringExtra(key);
            if (value != null) again.putExtra(key, value);
        }
        for (String key : new String[] {Game.EXTRA_APP_HDR, Game.EXTRA_MONITOR_RESUME}) {
            if (incoming.hasExtra(key)) again.putExtra(key, incoming.getBooleanExtra(key, false));
        }
        String name = incoming.getStringExtra(AppView.NAME_EXTRA);
        return base(context, Purpose.TRAMPOLINE).putExtra(EXTRA_NEXT, again)
                .putExtra(EXTRA_PC_UUID, uuid).putExtra(EXTRA_PC_NAME, name);
    }

    /** Checks the current PIN and answers RESULT_OK. */
    public static Intent forVerify(Context context) {
        return base(context, Purpose.VERIFY);
    }

    /** Chooses (twice) and stores a new PIN, answering RESULT_OK. */
    public static Intent forSet(Context context) {
        return base(context, Purpose.SET);
    }

    private Purpose purpose;
    private PinStore store;
    private final byte[] entry = new byte[PinStore.MAX_DIGITS];
    private int count;
    /** SET: the first entry, waiting for its confirmation. */
    private byte[] first;
    private boolean busy;
    private int dotCount;
    private int row;
    private boolean ringVisible;
    private long lockedUntilElapsed;

    private TextView title;
    private TextView subtitle;
    private TextView status;
    private View hint;
    private LinearLayout dots;
    private final View[] rows = new View[PinKeys.ROWS.length];
    private final MaterialButton[] keys = new MaterialButton[12];
    private ExecutorService worker;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final DpadEchoFilter echoes = new DpadEchoFilter();
    private final Runnable countdown = this::refreshLockout;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        UiHelper.setLocale(this);
        String name = getIntent().getStringExtra(EXTRA_PURPOSE);
        try {
            purpose = Purpose.valueOf(name == null ? "" : name);
        } catch (IllegalArgumentException e) {
            purpose = Purpose.APP;
        }
        store = new PinStore(this);
        if (purpose == Purpose.APP) PinGate.setShowing(true);
        worker = Executors.newSingleThreadExecutor();
        setContentView(R.layout.activity_pin_lock);
        title = findViewById(R.id.pin_title);
        subtitle = findViewById(R.id.pin_subtitle);
        status = findViewById(R.id.pin_status);
        hint = findViewById(R.id.pin_hint);
        dots = findViewById(R.id.pin_dots);
        rows[0] = findViewById(R.id.pin_row_0);
        rows[1] = findViewById(R.id.pin_row_1);
        rows[2] = findViewById(R.id.pin_row_2);
        rows[3] = findViewById(R.id.pin_row_3);
        for (int i = 0; i < rows.length; i++) {
            rows[i].setContentDescription(getString(new int[] {R.string.pin_row_1, R.string.pin_row_2,
                    R.string.pin_row_3, R.string.pin_row_4}[i]));
        }
        dotCount = purpose == Purpose.SET || !store.hasPin() ? PinStore.MAX_DIGITS : store.pinLength();
        buildDots();
        buildKeys();

        MaterialButton cancel = findViewById(R.id.pin_cancel);
        cancel.setText(purpose == Purpose.APP ? R.string.pin_close_app : R.string.pin_cancel);
        cancel.setOnClickListener(v -> leave());
        getOnBackPressedDispatcher().addCallback(this, new OnBackPressedCallback(true) {
            @Override public void handleOnBackPressed() {
                back();
            }
        });

        ringVisible = (getResources().getConfiguration().uiMode & Configuration.UI_MODE_TYPE_MASK)
                == Configuration.UI_MODE_TYPE_TELEVISION;
        render();
        updateRows();
        UiHelper.notifyNewRootView(this);
        if (purpose != Purpose.SET && !store.hasPin()) {
            // The PIN went away since this screen was asked for: nothing is left to check.
            handler.post(this::unlocked);
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (purpose == Purpose.APP && !PinGate.needsPinForApp(this)) {
            finish();
            return;
        }
        refreshLockout();
    }

    @Override
    protected void onPause() {
        echoes.reset();
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        handler.removeCallbacksAndMessages(null);
        Arrays.fill(entry, (byte) 0);
        if (first != null) Arrays.fill(first, (byte) 0);
        first = null;
        if (purpose == Purpose.APP) PinGate.setShowing(false);
        if (worker != null) worker.shutdown();
        super.onDestroy();
    }

    // ---- Views ----

    private void buildDots() {
        int size = getResources().getDimensionPixelSize(R.dimen.pin_dot_size);
        int gap = getResources().getDimensionPixelSize(R.dimen.space_s);
        dots.removeAllViews();
        for (int i = 0; i < dotCount; i++) {
            View dot = new View(this);
            dot.setBackgroundResource(R.drawable.pin_dot);
            dot.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
            LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(size, size);
            params.setMarginStart(gap / 2);
            params.setMarginEnd(gap / 2);
            dots.addView(dot, params);
        }
    }

    private void buildKeys() {
        int[] ids = {R.id.pin_key_0, R.id.pin_key_1, R.id.pin_key_2, R.id.pin_key_3, R.id.pin_key_4, R.id.pin_key_5,
                R.id.pin_key_6, R.id.pin_key_7, R.id.pin_key_8, R.id.pin_key_9, R.id.pin_key_del, R.id.pin_key_ok};
        int iconSize = getResources().getDimensionPixelSize(R.dimen.pin_key_text);
        for (int i = 0; i < ids.length; i++) {
            MaterialButton key = findViewById(ids[i]);
            keys[i] = key;
            if (i < 10) {
                final int digit = i;
                key.setText(String.format(Locale.getDefault(), "%d", digit));
                key.setOnClickListener(v -> touched(() -> type(digit)));
            } else {
                boolean delete = i == 10;
                key.setIcon(getDrawable(delete ? R.drawable.ic_backspace : R.drawable.ic_check));
                key.setIconSize(iconSize);
                key.setIconPadding(0);
                key.setIconGravity(MaterialButton.ICON_GRAVITY_TEXT_START);
                key.setOnClickListener(v -> touched(delete ? this::delete : this::submit));
            }
        }
    }

    /** A touch on the pad: the row highlight is for remotes, so it goes away. */
    private void touched(Runnable action) {
        if (ringVisible) {
            ringVisible = false;
            updateRows();
        }
        action.run();
    }

    private void updateRows() {
        for (int i = 0; i < rows.length; i++) rows[i].setActivated(ringVisible && i == row);
        hint.setVisibility(ringVisible ? View.VISIBLE : View.GONE);
    }

    private void updateDots() {
        for (int i = 0; i < dots.getChildCount(); i++) dots.getChildAt(i).setActivated(i < count);
        dots.setContentDescription(getString(R.string.pin_dots_description, count, dotCount));
    }

    private void render() {
        int titleRes;
        CharSequence sub;
        switch (purpose) {
            case SET:
                titleRes = first == null ? R.string.pin_title_choose : R.string.pin_title_confirm;
                sub = getString(first == null ? R.string.pin_subtitle_choose : R.string.pin_subtitle_confirm);
                break;
            case VERIFY:
                titleRes = R.string.pin_title_enter;
                sub = getString(R.string.pin_subtitle_verify);
                break;
            case STREAM:
            case TRAMPOLINE:
                titleRes = R.string.pin_title_enter;
                String pc = getIntent().getStringExtra(EXTRA_PC_NAME);
                sub = pc == null || pc.isEmpty() ? getString(R.string.pin_subtitle_stream)
                        : getString(R.string.pin_subtitle_stream_pc, pc);
                break;
            default:
                titleRes = R.string.pin_title_app;
                sub = getString(R.string.pin_subtitle_app);
        }
        title.setText(titleRes);
        subtitle.setText(sub);
        updateDots();
    }

    private void setStatus(CharSequence text, boolean error) {
        status.setText(text);
        status.setTextColor(MaterialColors.getColor(status, error
                ? androidx.appcompat.R.attr.colorError
                : com.google.android.material.R.attr.colorOnSurfaceVariant));
    }

    // ---- Input ----

    private boolean inputAllowed() {
        return !busy && lockedUntilElapsed == 0;
    }

    void type(int digit) {
        if (!inputAllowed() || count >= dotCount) return;
        entry[count++] = (byte) ('0' + digit);
        if (status.getText().length() > 0 && lockedUntilElapsed == 0) setStatus("", false);
        updateDots();
        if (purpose != Purpose.SET && count == store.pinLength()) submit();
    }

    void delete() {
        if (!inputAllowed() || count == 0) return;
        entry[--count] = 0;
        updateDots();
    }

    private void clearEntry() {
        Arrays.fill(entry, (byte) 0);
        count = 0;
        updateDots();
    }

    void submit() {
        if (!inputAllowed()) return;
        if (purpose == Purpose.SET) {
            submitNew();
            return;
        }
        if (count == 0) return;
        byte[] pin = Arrays.copyOf(entry, count);
        clearEntry();
        busy = true;
        setStatus(getString(R.string.pin_checking), false);
        executor().execute(() -> {
            PinStore.Attempt attempt = store.tryPin(pin);
            Arrays.fill(pin, (byte) 0);
            runOnUiThread(() -> attempted(attempt));
        });
    }

    private Executor executor() {
        return executorForTests != null ? executorForTests : worker;
    }

    private void attempted(PinStore.Attempt attempt) {
        busy = false;
        if (isDestroyed()) return;
        switch (attempt.result) {
            case OK:
                unlocked();
                break;
            case WRONG:
                setStatus(getString(R.string.pin_wrong), true);
                refreshLockout();
                break;
            default:
                refreshLockout();
        }
    }

    private void submitNew() {
        if (count < PinStore.MIN_DIGITS) {
            setStatus(getString(R.string.pin_too_short), true);
            return;
        }
        if (first == null) {
            first = Arrays.copyOf(entry, count);
            clearEntry();
            setStatus("", false);
            render();
            return;
        }
        byte[] again = Arrays.copyOf(entry, count);
        boolean same = PinHasher.constantTimeEquals(first, again);
        clearEntry();
        if (!same) {
            Arrays.fill(again, (byte) 0);
            Arrays.fill(first, (byte) 0);
            first = null;
            setStatus(getString(R.string.pin_mismatch), true);
            render();
            return;
        }
        byte[] chosen = first;
        first = null;
        busy = true;
        setStatus(getString(R.string.pin_saving), false);
        executor().execute(() -> {
            store.setPin(chosen);
            Arrays.fill(chosen, (byte) 0);
            Arrays.fill(again, (byte) 0);
            runOnUiThread(() -> {
                busy = false;
                if (isDestroyed()) return;
                PinGate.unlockApp();
                Toast.makeText(this, R.string.pin_saved, Toast.LENGTH_SHORT).show();
                setResult(RESULT_OK);
                finish();
            });
        });
    }

    /** Shows the lockout countdown while one is running and keeps the keys off until it ends. */
    private void refreshLockout() {
        handler.removeCallbacks(countdown);
        long remaining = store.hasPin() ? store.lockedForMs() : 0;
        if (remaining > 0) {
            lockedUntilElapsed = store.elapsed() + remaining;
            long seconds = (remaining + 999) / 1000;
            setStatus(getString(R.string.pin_locked_out,
                    String.format(Locale.getDefault(), "%d:%02d", seconds / 60, seconds % 60)), true);
            handler.postDelayed(countdown, 1000);
        } else if (lockedUntilElapsed != 0) {
            lockedUntilElapsed = 0;
            setStatus("", false);
        }
    }

    private void unlocked() {
        if (isFinishing()) return;
        Intent next = purpose == Purpose.STREAM || purpose == Purpose.TRAMPOLINE ? nextIntent() : null;
        switch (purpose) {
            case APP:
                PinGate.unlockApp();
                break;
            case TRAMPOLINE:
                if (store.mode() == PinStore.Mode.APP) PinGate.unlockApp();
                PinGate.grant(getIntent().getStringExtra(EXTRA_PC_UUID));
                break;
            case VERIFY:
                setResult(RESULT_OK);
                break;
            default:
                break;
        }
        if (next != null) startActivity(next);
        finish();
    }

    @SuppressWarnings("deprecation")
    private Intent nextIntent() {
        return getIntent().getParcelableExtra(EXTRA_NEXT);
    }

    /** Leaves without the PIN: the app closes, anything else just goes back. */
    private void leave() {
        if (purpose == Purpose.APP) {
            finishAffinity();
        } else {
            setResult(RESULT_CANCELED);
            finish();
        }
    }

    /** Back and the pad's B button take back the last digit if a remote is typing, and leave otherwise. */
    private void back() {
        if (ringVisible && count > 0 && inputAllowed()) {
            delete();
        } else {
            leave();
        }
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        if (echoes.drop(event)) return true;
        boolean keyboard = false;
        InputDevice device = event.getDevice();
        if (device != null) {
            keyboard = device.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC
                    && (event.getSource() & InputDevice.SOURCE_KEYBOARD) == InputDevice.SOURCE_KEYBOARD;
        }
        PinKeys.Action action = PinKeys.of(event.getKeyCode(), keyboard);
        if (action.kind == PinKeys.Kind.NONE) return super.dispatchKeyEvent(event);
        if (event.getAction() == KeyEvent.ACTION_DOWN) {
            boolean moves = action.kind == PinKeys.Kind.ROW_UP || action.kind == PinKeys.Kind.ROW_DOWN;
            if (event.getRepeatCount() == 0 || moves) perform(action);
        }
        return true;
    }

    private void perform(PinKeys.Action action) {
        switch (action.kind) {
            case ROW_UP:
            case ROW_DOWN:
                ringVisible = true;
                row = PinKeys.moveRow(row, action.kind == PinKeys.Kind.ROW_DOWN);
                updateRows();
                rows[row].announceForAccessibility(rows[row].getContentDescription());
                break;
            case CELL:
                ringVisible = true;
                updateRows();
                int cell = PinKeys.cell(row, action.value);
                if (cell == PinKeys.DELETE) delete();
                else if (cell == PinKeys.CONFIRM) submit();
                else type(cell);
                break;
            case DIGIT:
                type(action.value);
                break;
            case DELETE:
                delete();
                break;
            case SUBMIT:
                submit();
                break;
            case BACK:
                back();
                break;
            default:
                break;
        }
    }
}
