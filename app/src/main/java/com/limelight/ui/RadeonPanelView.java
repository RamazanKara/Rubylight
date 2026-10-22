package com.limelight.ui;

import android.content.res.Resources;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.view.View;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.core.view.ViewCompat;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.slider.Slider;
import com.limelight.R;
import com.limelight.console.NativeMenuController;
import com.limelight.console.RadeonPanelModel;
import com.limelight.nativeapi.NativeRadeon;

import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * Draws the Radeon panel of the stream menu into a group of the sheet and redraws it when the state
 * changes. It draws exactly what {@link RadeonPanelModel} decided: a control only where the host lets the
 * setting be written now, otherwise the value and the reason. The panel is rebuilt only when what it shows
 * changed, keeps the focused row focused, and is left alone while a slider is being moved.
 */
final class RadeonPanelView {
    interface Writer {
        void write(RadeonPanelModel.Row row, Object value);
    }

    /** A slider that stays still this long after its last key press is written. */
    static final long KEY_COMMIT_DELAY_MS = 600;

    private final ActionSheet sheet;
    private final LinearLayout group;
    private final Writer writer;
    private final Resources resources;
    private final Map<String, Object> pendingValues = new HashMap<>();

    private NativeMenuController.State shown;
    private NativeMenuController.State deferred;
    private boolean editing;
    private Runnable keyCommit;
    private int rebuilds;

    RadeonPanelView(ActionSheet sheet, LinearLayout group, Writer writer) {
        this.sheet = sheet;
        this.group = group;
        this.writer = writer;
        this.resources = group.getResources();
    }

    /** How many times the panel was drawn from scratch; a steady panel stays at the same count. */
    int rebuilds() {
        return rebuilds;
    }

    void detach() {
        if (keyCommit != null) {
            group.removeCallbacks(keyCommit);
            keyCommit = null;
        }
        editing = false;
    }

    void show(NativeMenuController.State state) {
        if (editing) {
            deferred = state;
            return;
        }
        deferred = null;
        if (state.hidden) {
            shown = state;
            group.removeAllViews();
            group.setVisibility(View.GONE);
            return;
        }
        if (sameView(shown, state)) {
            shown = state;
            return;
        }
        if (state.pendingId == null) {
            pendingValues.clear();
        }
        shown = state;
        rebuild(state);
    }

    private static boolean sameView(NativeMenuController.State a, NativeMenuController.State b) {
        return a != null && !a.hidden && a.model == b.model && Objects.equals(a.loadFailure, b.loadFailure)
                && Objects.equals(a.writeFailure, b.writeFailure) && Objects.equals(a.pendingId, b.pendingId)
                && a.loaded() == b.loaded();
    }

    private void rebuild(NativeMenuController.State state) {
        rebuilds++;
        View focused = group.findFocus();
        Object focusTag = focused == null ? null : focused.getTag();
        group.removeAllViews();
        group.setVisibility(View.VISIBLE);
        sheet.addSection(group, R.string.native_section_radeon);
        RadeonPanelModel model = state.model;
        if (model == null) {
            sheet.addNote(group, state.loadFailure != null ? failureText(state.loadFailure)
                    : resources.getString(R.string.native_loading), state.loadFailure != null);
            return;
        }
        int notice = noticeText(model.notice);
        if (notice != 0) {
            sheet.addNote(group, resources.getString(notice), false);
        }
        if (model.message != null && !model.message.isEmpty()) {
            sheet.addNote(group, model.message, false);
        }
        RadeonPanelModel.Failure failure = state.writeFailure;
        if (failure != null && (failure.settingId == null || model.find(failure.settingId) == null)) {
            sheet.addNote(group, failureText(failure), true);
        }
        for (RadeonPanelModel.Gpu gpu : model.gpus) {
            String name = gpu.name == null ? "" : gpu.name;
            if (gpu.driver != null && !gpu.driver.isEmpty()) {
                name = name.isEmpty() ? gpu.driver : name + " · " + gpu.driver;
            }
            if (!name.isEmpty()) {
                sheet.addNote(group, name, false);
            }
            for (RadeonPanelModel.Row row : gpu.rows) {
                addRow(row, state);
            }
        }
        if (focusTag != null) {
            View again = group.findViewWithTag(focusTag);
            if (again != null) {
                again.requestFocus();
            }
        }
    }

    private void addRow(RadeonPanelModel.Row row, NativeMenuController.State state) {
        String title = row.title(resources);
        boolean pending = row.id.equals(state.pendingId);
        Object value = pending && pendingValues.containsKey(row.id) ? pendingValues.get(row.id) : row.value;
        switch (row.mode) {
            case EDIT:
                if (row.kind == NativeRadeon.Kind.INTEGER) {
                    addSlider(row, title, value);
                } else if (row.kind == NativeRadeon.Kind.CHOICE) {
                    addChoice(row, title, value);
                } else {
                    addToggle(row, title, Boolean.TRUE.equals(value));
                }
                break;
            case VALUE:
                sheet.addInfo(group, title, valueText(row, value));
                break;
            default:
                String shownValue = row.value == null ? "" : valueText(row, row.value);
                sheet.addInfo(group, title, shownValue);
                break;
        }
        String note = null;
        boolean error = false;
        if (state.writeFailure != null && row.id.equals(state.writeFailure.settingId)) {
            note = failureText(state.writeFailure);
            error = true;
        } else if (pending) {
            note = resources.getString(R.string.native_applying);
        } else if (row.mode == RadeonPanelModel.Mode.UNSUPPORTED || row.mode == RadeonPanelModel.Mode.UNAVAILABLE
                || row.mode == RadeonPanelModel.Mode.EXTERNAL_SETUP) {
            note = row.message != null && !row.message.isEmpty() ? row.message : resources.getString(
                    row.mode == RadeonPanelModel.Mode.UNSUPPORTED ? R.string.native_row_unsupported
                            : row.mode == RadeonPanelModel.Mode.UNAVAILABLE ? R.string.native_row_unavailable
                            : R.string.native_row_external);
        }
        if (note != null) {
            sheet.addNote(group, note, error);
        }
        if (row.restore != null && !error) {
            sheet.addNote(group, resources.getString(R.string.native_restore_hint,
                    RadeonPanelModel.restoreText(row, row.restore, resources.getString(R.string.native_on),
                            resources.getString(R.string.native_off))), false);
        }
    }

    private String valueText(RadeonPanelModel.Row row, Object value) {
        if (value instanceof Boolean) {
            return resources.getString((Boolean) value ? R.string.native_on : R.string.native_off);
        }
        if (value == null) {
            return resources.getString(R.string.native_row_unavailable);
        }
        if (value instanceof String) {
            for (NativeRadeon.Choice choice : row.options) {
                if (choice.value.equals(value)) {
                    return choice.label == null || choice.label.isEmpty() ? choice.value : choice.label;
                }
            }
        }
        return RadeonPanelModel.withUnit(String.valueOf(value), row.unit);
    }

    private void addToggle(RadeonPanelModel.Row row, String title, boolean on) {
        MaterialButton button = sheet.addAction(group, R.drawable.ic_check, title);
        button.setTag(row.id);
        sheet.setActionText(button, title, resources.getString(on ? R.string.native_on : R.string.native_off));
        if (!on) {
            button.setIconTint(ColorStateList.valueOf(Color.TRANSPARENT));
        }
        ViewCompat.setStateDescription(button, resources.getString(on ? R.string.native_on : R.string.native_off));
        button.setOnClickListener(view -> send(row, !on));
    }

    private void addChoice(RadeonPanelModel.Row row, String title, Object value) {
        MaterialButton button = sheet.addAction(group, R.drawable.ic_tune, title);
        button.setTag(row.id);
        sheet.setActionText(button, title, valueText(row, value));
        List<NativeRadeon.Choice> options = row.options;
        button.setOnClickListener(view -> {
            // A tap moves to the next option, so a choice needs neither a list nor a second dialog.
            int at = -1;
            for (int i = 0; i < options.size(); i++) {
                if (options.get(i).value.equals(value)) {
                    at = i;
                }
            }
            send(row, options.get((at + 1) % options.size()).value);
        });
    }

    private void addSlider(RadeonPanelModel.Row row, String title, Object value) {
        NativeRadeon.Range range = row.range;
        long from = range.min;
        long to = range.max;
        long step = Math.max(1, range.step);
        if ((to - from) % step != 0) {
            // The slider needs the range to be whole steps; the host's own check still decides.
            step = 1;
        }
        long start = value instanceof Number ? ((Number) value).longValue() : from;
        start = (Long) RadeonPanelModel.writableValue(row, start);
        LinearLayout box = new LinearLayout(group.getContext());
        box.setOrientation(LinearLayout.VERTICAL);
        int pad = sheet.spacing();
        box.setPadding(pad, pad / 2, pad, 0);
        LinearLayout line = new LinearLayout(group.getContext());
        TextView name = new TextView(group.getContext());
        name.setText(title);
        name.setTextAppearance(group.getContext(), R.style.TextAppearance_Nightjar_Body);
        TextView number = new TextView(group.getContext());
        number.setText(RadeonPanelModel.withUnit(String.valueOf(start), row.unit));
        number.setTextAppearance(group.getContext(), R.style.TextAppearance_Nightjar_Body);
        number.setGravity(android.view.Gravity.END);
        line.addView(name, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1));
        line.addView(number, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        Slider slider = new Slider(group.getContext());
        slider.setTag(row.id);
        slider.setValueFrom(from);
        slider.setValueTo(to);
        slider.setStepSize(step);
        slider.setValue(Math.max(from, Math.min(to, start)));
        slider.setLabelBehavior(com.google.android.material.slider.LabelFormatter.LABEL_GONE);
        slider.setContentDescription(title);
        final boolean[] touching = {false};
        slider.addOnChangeListener((changed, current, fromUser) -> {
            if (!fromUser) {
                return;
            }
            number.setText(RadeonPanelModel.withUnit(String.valueOf((long) current), row.unit));
            editing = true;
            if (keyCommit != null) {
                group.removeCallbacks(keyCommit);
            }
            if (!touching[0]) {
                keyCommit = () -> commit(row, slider);
                group.postDelayed(keyCommit, KEY_COMMIT_DELAY_MS);
            }
        });
        slider.addOnSliderTouchListener(new Slider.OnSliderTouchListener() {
            @Override
            public void onStartTrackingTouch(Slider touched) {
                touching[0] = true;
                editing = true;
                if (keyCommit != null) {
                    group.removeCallbacks(keyCommit);
                    keyCommit = null;
                }
            }

            @Override
            public void onStopTrackingTouch(Slider touched) {
                touching[0] = false;
                commit(row, touched);
            }
        });
        box.addView(line);
        box.addView(slider, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        group.addView(box, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
    }

    private void commit(RadeonPanelModel.Row row, Slider slider) {
        if (keyCommit != null) {
            group.removeCallbacks(keyCommit);
            keyCommit = null;
        }
        editing = false;
        send(row, (long) slider.getValue());
    }

    private void send(RadeonPanelModel.Row row, Object value) {
        pendingValues.put(row.id, value);
        writer.write(row, value);
    }

    private static int noticeText(RadeonPanelModel.Notice notice) {
        switch (notice) {
            case READ_ONLY: return R.string.native_notice_read_only;
            case UNSUPPORTED: return R.string.native_notice_unsupported;
            case UNAVAILABLE: return R.string.native_notice_unavailable;
            case ADLX_UNAVAILABLE: return R.string.native_notice_adlx;
            case NEEDS_EXTERNAL_SETUP: return R.string.native_notice_external;
            case NEEDS_HOST_CONTROL: return R.string.native_notice_host_control;
            case NEEDS_SESSION: return R.string.native_notice_session;
            default: return 0;
        }
    }

    private String failureText(RadeonPanelModel.Failure failure) {
        if (failure.messageRes == R.string.native_error_generic && failure.hostMessage != null
                && !failure.hostMessage.isEmpty()) {
            return failure.hostMessage;
        }
        return resources.getString(failure.messageRes);
    }
}
