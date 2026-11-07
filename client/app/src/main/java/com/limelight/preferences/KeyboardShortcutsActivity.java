package com.limelight.preferences;

import android.app.Application;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.text.InputType;
import android.view.MotionEvent;
import android.view.View;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;
import com.limelight.ui.NightjarActivity;
import androidx.lifecycle.AndroidViewModel;
import androidx.lifecycle.MutableLiveData;
import androidx.lifecycle.ViewModelProvider;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.R;
import com.limelight.binding.input.KeyboardShortcuts;
import com.limelight.binding.input.KeyboardShortcuts.Shortcut;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.UUID;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** Edits chords without sending any input; imports are bounded and reviewed before replacement. */
public final class KeyboardShortcutsActivity extends NightjarActivity {
    /** A TV lays out Settings' screens at {@link com.limelight.ui.AppTheme#TV_COMPACT_PERCENT} of the interface size. */
    @Override
    protected boolean compactOnTelevision() {
        return true;
    }

    private static final int IMPORT = 1;
    private static final int EXPORT = 2;
    private static final String STATE_DRAFT = "shortcutDraft";
    private LinearLayout content;
    private ShortcutWork work;
    private AlertDialog importDialog;
    private AlertDialog editDialog;
    private EditText editName, editChord;
    private String editId;

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        UiHelper.setLocale(this);
        android.widget.FrameLayout page = TopAppBar.setUpPage(this, R.string.keyboard_shortcuts_title, this::finish);
        content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        TopAppBar.addScrollingBody(page, content);
        work = new ViewModelProvider(this).get(ShortcutWork.class);
        work.busy.observe(this, ignored -> refresh());
        work.message.observe(this, ignored -> refresh());
        work.imported.observe(this, entries -> {
            if (entries == null || importDialog != null) return;
            importDialog = new NavigationDialogBuilder(this)
                    .setTitle(R.string.keyboard_shortcuts_import)
                    .setMessage(getString(R.string.keyboard_shortcuts_replace, entries.size()))
                    .setPositiveButton(R.string.keyboard_shortcuts_replace_button, (dialog, which) -> {
                        KeyboardShortcuts.save(this, entries);
                        work.imported.setValue(null);
                        refresh();
                    })
                    .setNegativeButton(android.R.string.cancel, (dialog, which) -> work.imported.setValue(null))
                    .setOnCancelListener(dialog -> work.imported.setValue(null))
                    .create();
            importDialog.setOnDismissListener(dialog -> importDialog = null);
            importDialog.show();
        });
        refresh();
        Bundle draft = state == null ? null : state.getBundle(STATE_DRAFT);
        if (draft != null) {
            Shortcut existing = null;
            String id = draft.getString("id");
            for (Shortcut entry : KeyboardShortcuts.load(this)) {
                if (entry.id.equals(id)) existing = entry;
            }
            if (id == null || existing != null) {
                edit(existing);
                editName.setText(draft.getString("name", ""));
                editChord.setText(draft.getString("chord", ""));
            }
        }
        UiHelper.notifyNewRootView(this);
    }

    private void refresh() {
        content.removeAllViews();
        TextView intro = new TextView(this);
        intro.setTextAppearance(this, R.style.TextAppearance_Nightjar_Body);
        intro.setText(R.string.keyboard_shortcuts_help);
        content.addView(intro);
        boolean busy = Boolean.TRUE.equals(work.busy.getValue());
        List<Shortcut> entries = KeyboardShortcuts.load(this);
        button(com.google.android.material.R.attr.materialButtonStyle, getString(R.string.keyboard_shortcuts_add), !busy && entries.size() < KeyboardShortcuts.MAX_SHORTCUTS,
                () -> edit(null));
        button(R.attr.nightjarButtonTonalStyle, getString(R.string.keyboard_shortcuts_import), !busy, () -> chooseFile(new Intent(Intent.ACTION_OPEN_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE).setType("application/json")
                .putExtra(Intent.EXTRA_MIME_TYPES, new String[] {"application/json", "text/plain", "application/octet-stream"}), IMPORT));
        button(R.attr.nightjarButtonTonalStyle, getString(R.string.keyboard_shortcuts_export), !busy, () -> chooseFile(new Intent(Intent.ACTION_CREATE_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE).setType("application/json")
                .putExtra(Intent.EXTRA_TITLE, "nightjar-keyboard-shortcuts.json"), EXPORT));
        if (work.message.getValue() != null) {
            TextView message = new TextView(this);
            message.setTextAppearance(this, R.style.TextAppearance_Nightjar_Supporting);
            message.setText(work.message.getValue());
            content.addView(message);
        }
        LinearLayout rows = new LinearLayout(this);
        rows.setOrientation(LinearLayout.VERTICAL);
        content.addView(rows);
        for (Shortcut entry : entries) rows.addView(shortcutRow(entry, !busy));
        com.limelight.ui.SubPageFocus.apply(this, findViewById(R.id.page_content));
    }

    /** A shortcut's row: its handle, which drags the row or moves it with up and down, and the button that edits it. */
    @android.annotation.SuppressLint("ClickableViewAccessibility") // The handle's click is only for focus; dragging has the keys' way too.
    private LinearLayout shortcutRow(Shortcut entry, boolean enabled) {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(android.view.Gravity.CENTER_VERTICAL);
        MaterialButton handle = new MaterialButton(this, null, R.attr.nightjarButtonTextStyle);
        handle.setIconResource(R.drawable.ic_drag_handle);
        handle.setIconPadding(0);
        handle.setContentDescription(getString(R.string.keyboard_shortcuts_reorder));
        handle.setTag(entry.id);
        handle.setEnabled(enabled);
        handle.setOnTouchListener(new RowDrag(row, entry.id));
        handle.setOnKeyListener((view, keyCode, event) -> {
            if (keyCode != android.view.KeyEvent.KEYCODE_DPAD_UP && keyCode != android.view.KeyEvent.KEYCODE_DPAD_DOWN) return false;
            int step = keyCode == android.view.KeyEvent.KEYCODE_DPAD_UP ? -1 : 1;
            LinearLayout rows = (LinearLayout) row.getParent();
            int to = rows.indexOfChild(row) + step;
            // At either end the focus moves on, as from any other control.
            if (to < 0 || to >= rows.getChildCount()) return false;
            if (event.getAction() == android.view.KeyEvent.ACTION_DOWN) {
                KeyboardShortcuts.move(this, entry.id, to);
                refresh();
                View moved = content.findViewWithTag(entry.id);
                if (moved != null) moved.requestFocus();
            }
            return true;
        });
        row.addView(handle, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT));
        MaterialButton edit = new MaterialButton(this, null, R.attr.nightjarButtonTextStyle);
        edit.setText(entry.name + "\n" + KeyboardShortcuts.format(entry.keys()));
        edit.setAllCaps(false);
        edit.setEnabled(enabled);
        edit.setGravity(android.view.Gravity.START | android.view.Gravity.CENTER_VERTICAL);
        edit.setOnClickListener(view -> edit(entry));
        row.addView(edit, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        return row;
    }

    /**
     * Drags a row by its handle: the row follows the finger and swaps places with the neighbour it passes, and the new
     * order is saved when the finger lifts. The neighbours move and not the row, since a removed view loses the touch.
     */
    private final class RowDrag implements View.OnTouchListener {
        private final LinearLayout row;
        private final String id;
        private float lastY;

        RowDrag(LinearLayout row, String id) {
            this.row = row;
            this.id = id;
        }

        @Override public boolean onTouch(View handle, MotionEvent event) {
            LinearLayout rows = (LinearLayout) row.getParent();
            if (rows == null || !handle.isEnabled()) return false;
            switch (event.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    lastY = event.getRawY();
                    rows.requestDisallowInterceptTouchEvent(true);
                    row.setElevation(8 * getResources().getDisplayMetrics().density);
                    return true;
                case MotionEvent.ACTION_MOVE:
                    row.setTranslationY(row.getTranslationY() + event.getRawY() - lastY);
                    lastY = event.getRawY();
                    swapPassed(rows);
                    return true;
                case MotionEvent.ACTION_UP:
                    row.setElevation(0);
                    row.setTranslationY(0);
                    KeyboardShortcuts.move(KeyboardShortcutsActivity.this, id, rows.indexOfChild(row));
                    // The rebuild waits for the touch's dispatch to end: inside it the tree the page walks had a null child.
                    content.post(KeyboardShortcutsActivity.this::refresh);
                    return true;
                case MotionEvent.ACTION_CANCEL:
                    content.post(KeyboardShortcutsActivity.this::refresh);
                    return true;
                default:
                    return false;
            }
        }

        private void swapPassed(LinearLayout rows) {
            int index = rows.indexOfChild(row);
            float center = row.getTop() + row.getTranslationY() + row.getHeight() / 2f;
            int passed = passedIndex(index, center, rows);
            if (passed == index) return;
            // The row keeps its place on screen while the layout puts it in the new slot.
            View neighbour = rows.getChildAt(passed);
            int newTop = passed > index ? neighbour.getBottom() - row.getHeight() : neighbour.getTop();
            row.setTranslationY(row.getTop() + row.getTranslationY() - newTop);
            rows.removeViewAt(passed);
            rows.addView(neighbour, index);
        }
    }

    /** The neighbour of {@code index} whose middle a row centred at {@code center} has passed, or {@code index}. */
    private static int passedIndex(int index, float center, LinearLayout rows) {
        if (index + 1 < rows.getChildCount()) {
            View next = rows.getChildAt(index + 1);
            if (center > next.getTop() + next.getHeight() / 2f) return index + 1;
        }
        if (index > 0) {
            View previous = rows.getChildAt(index - 1);
            if (center < previous.getTop() + previous.getHeight() / 2f) return index - 1;
        }
        return index;
    }

    private MaterialButton button(int styleAttr, String text, boolean enabled, Runnable action) {
        MaterialButton button = new MaterialButton(this, null, styleAttr);
        button.setText(text);
        button.setAllCaps(false);
        button.setEnabled(enabled);
        button.setOnClickListener(view -> action.run());
        content.addView(button);
        return button;
    }

    private void chooseFile(Intent intent, int request) {
        try {
            startActivityForResult(intent, request);
        } catch (ActivityNotFoundException e) {
            work.message.setValue(getString(R.string.keyboard_shortcuts_file_error));
        }
    }

    private void edit(Shortcut existing) {
        if (editDialog != null) return;
        LinearLayout fields = new LinearLayout(this);
        fields.setOrientation(LinearLayout.VERTICAL);
        int padding = Math.round(24 * getResources().getDisplayMetrics().density);
        fields.setPadding(padding, 0, padding, 0);
        EditText name = new EditText(this);
        name.setHint(R.string.keyboard_shortcuts_name);
        name.setSingleLine(true);
        name.setFilters(new android.text.InputFilter[] {new android.text.InputFilter.LengthFilter(80)});
        fields.addView(name);
        EditText chord = new EditText(this);
        chord.setHint(R.string.keyboard_shortcuts_chord);
        chord.setSingleLine(true);
        chord.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        chord.setFilters(new android.text.InputFilter[] {new android.text.InputFilter.LengthFilter(256)});
        fields.addView(chord);
        if (existing != null) {
            name.setText(existing.name);
            chord.setText(KeyboardShortcuts.format(existing.keys()));
        }
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(this)
                .setTitle(existing == null ? R.string.keyboard_shortcuts_add : R.string.keyboard_shortcuts_edit)
                .setView(fields).setPositiveButton(R.string.keyboard_shortcuts_save, null)
                .setNegativeButton(android.R.string.cancel, null);
        if (existing != null) builder.setNeutralButton(R.string.keyboard_shortcuts_delete, (dialog, which) -> {
            List<Shortcut> entries = KeyboardShortcuts.load(this);
            entries.removeIf(entry -> entry.id.equals(existing.id));
            KeyboardShortcuts.save(this, entries);
            refresh();
        });
        AlertDialog dialog = builder.create();
        editDialog = dialog;
        editName = name;
        editChord = chord;
        editId = existing == null ? null : existing.id;
        dialog.setOnDismissListener(ignored -> {
            editDialog = null;
            editName = editChord = null;
            editId = null;
        });
        dialog.setOnShowListener(ignored -> dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            try {
                Shortcut entry = new Shortcut(existing == null ? UUID.randomUUID().toString() : existing.id,
                        name.getText().toString(), KeyboardShortcuts.parseChord(chord.getText().toString()));
                List<Shortcut> entries = KeyboardShortcuts.load(this);
                int index = -1;
                for (int i = 0; i < entries.size(); i++) if (entries.get(i).id.equals(entry.id)) index = i;
                if (index >= 0) entries.set(index, entry);
                else entries.add(entry);
                KeyboardShortcuts.save(this, entries);
                dialog.dismiss();
                refresh();
            } catch (IllegalArgumentException e) {
                chord.setError(getString(R.string.keyboard_shortcuts_invalid));
            }
        }));
        dialog.show();
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        if (editDialog != null && editDialog.isShowing()) {
            Bundle draft = new Bundle();
            draft.putString("id", editId);
            draft.putString("name", editName.getText().toString());
            draft.putString("chord", editChord.getText().toString());
            state.putBundle(STATE_DRAFT, draft);
        }
        super.onSaveInstanceState(state);
    }

    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (result != RESULT_OK || data == null || data.getData() == null) return;
        if (request == IMPORT) work.importFile(data.getData());
        else if (request == EXPORT) work.exportFile(data.getData());
    }

    @Override protected void onDestroy() {
        if (importDialog != null) importDialog.dismiss();
        if (editDialog != null) editDialog.dismiss();
        super.onDestroy();
    }

    public static final class ShortcutWork extends AndroidViewModel {
        final MutableLiveData<Boolean> busy = new MutableLiveData<>(false);
        final MutableLiveData<String> message = new MutableLiveData<>();
        final MutableLiveData<List<Shortcut>> imported = new MutableLiveData<>();
        private final ExecutorService executor = Executors.newSingleThreadExecutor();

        public ShortcutWork(Application application) { super(application); }

        void importFile(Uri uri) {
            if (Boolean.TRUE.equals(busy.getValue())) return;
            busy.setValue(true);
            executor.execute(() -> {
                try (InputStream input = getApplication().getContentResolver().openInputStream(uri)) {
                    imported.postValue(KeyboardShortcuts.read(input));
                    message.postValue(null);
                } catch (IOException | IllegalArgumentException | SecurityException e) {
                    message.postValue(getApplication().getString(R.string.keyboard_shortcuts_file_error));
                } finally {
                    busy.postValue(false);
                }
            });
        }

        void exportFile(Uri uri) {
            if (Boolean.TRUE.equals(busy.getValue())) return;
            String json = KeyboardShortcuts.encode(KeyboardShortcuts.load(getApplication()));
            busy.setValue(true);
            executor.execute(() -> {
                try (OutputStream output = getApplication().getContentResolver().openOutputStream(uri, "wt")) {
                    if (output == null) throw new IOException("Cannot open shortcuts");
                    output.write(json.getBytes(StandardCharsets.UTF_8));
                    message.postValue(getApplication().getString(R.string.keyboard_shortcuts_exported));
                } catch (IOException | IllegalArgumentException | SecurityException e) {
                    message.postValue(getApplication().getString(R.string.keyboard_shortcuts_file_error));
                } finally {
                    busy.postValue(false);
                }
            });
        }

        @Override protected void onCleared() { executor.shutdownNow(); }
    }
}
