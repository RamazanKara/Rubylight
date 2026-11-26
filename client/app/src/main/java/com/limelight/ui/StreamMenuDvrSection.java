package com.limelight.ui;

import android.content.Context;
import android.view.View;
import android.widget.LinearLayout;

import com.google.android.material.button.MaterialButton;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nativeapi.KnownCapabilities;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;

import org.json.JSONException;

import java.util.Locale;
import java.util.function.Consumer;
import java.util.function.LongSupplier;

/**
 * The stream menu's Instant Replay group (Stream DVR, control message 0x554A): Save clip and Start / Stop recording as the
 * host's latest DVR state has them, the line under each (the buffer, the recording's time, or why it is off), and the toast
 * a clip result or a refusal shows. Offered only by a host with {@link HostCapabilities.Feature#STREAM_DVR} that announced
 * 0x554A, to the device that started the stream. The rows are plain actions, with no button hints.
 */
public final class StreamMenuDvrSection {
    /** The wire's ops. */
    public static final int OP_QUERY = 1, OP_SAVE = 2, OP_START = 3, OP_STOP = 4;
    /** The wire's states. */
    static final int OFF = 0, ARMED = 1, RECORDING = 2, SAVING = 3, UNSUPPORTED = 4, ERROR = 5;
    /** The wire's clip outcome for a saved clip; refused and failed carry a reason. */
    static final int SAVED = 0;
    /** The wire's reasons. */
    static final int NONE = 0, DISABLED = 1, PYROWAVE = 2, DISK_FULL = 3, QUOTA_EXCEEDED = 4, NO_KEYFRAME_YET = 5, BUSY = 6,
            PERMISSION_DENIED = 7, AUDIO_CODEC_UNSUPPORTED = 8, IO_ERROR = 9, NOT_STREAMING = 10, UNKNOWN = 11;
    /** A save or a stop the host has not answered in this long frees its row again. */
    static final long ANSWER_MS = 30_000;

    /** Sends one request; false when it could not go. */
    public interface Sender {
        boolean send(int requestId, int op, int durationSecs);
    }

    /** One row: what pressing it asks, its words, the line under it (null for none) and whether that line says why it is off. */
    static final class Row {
        final int op;
        final String label;
        final String detail;
        final boolean enabled, problem;

        Row(int op, String label, String detail, boolean enabled, boolean problem) {
            this.op = op;
            this.label = label;
            this.detail = detail;
            this.enabled = enabled;
            this.problem = problem;
        }
    }

    private final Context context;
    private final Sender sender;
    private final Consumer<CharSequence> toast;
    private final LongSupplier clock;
    private int lastId;
    private boolean known;
    private int state, reason, bufferedSecs, recordingSecs;
    private int pendingId, pendingOp;
    private long pendingAt;
    private ActionSheet sheet;
    private LinearLayout group;

    public StreamMenuDvrSection(Context context, Sender sender, Consumer<CharSequence> toast, LongSupplier clock) {
        this.context = context;
        this.sender = sender;
        this.toast = toast;
        this.clock = clock;
    }

    /** Whether the menu offers it: the host's feature, 0x554A in this stream's announcement, and the stream's own device. */
    public static boolean offered(boolean feature, boolean announced, NvApp.Role role) {
        return feature && announced && role == NvApp.Role.STREAM;
    }

    /** Whether {@code host} offers Stream DVR, with the native capabilities this process read for it; false while unread. */
    public static boolean hostOffers(ComputerDetails host) {
        String known = KnownCapabilities.of(host.uuid);
        if (known == null) return false;
        try {
            return host.caps.withNativeCapabilities(known).has(HostCapabilities.Feature.STREAM_DVR);
        } catch (JSONException e) {
            LimeLog.warning("Stream DVR: unreadable native capabilities: " + e.getMessage());
            return false;
        }
    }

    /** Asks the host for its state, once the stream runs. */
    public void start() {
        request(OP_QUERY);
    }

    void request(int op) {
        lastId = lastId == Integer.MAX_VALUE ? 1 : lastId + 1;
        if (op != OP_QUERY) {
            pendingId = lastId;
            pendingOp = op;
            pendingAt = clock.getAsLong();
        }
        if (!sender.send(lastId, op, 0) && op != OP_QUERY) {
            pendingOp = 0;
            toast.accept(reasonText(context, UNKNOWN));
        }
        update();
    }

    /** The host's state: an answer to a request (its id) or its own push (0). UI thread. */
    public void onState(int requestId, int state, int reason, int bufferedSecs, int recordingSecs) {
        known = true;
        this.state = state;
        this.reason = reason;
        this.bufferedSecs = bufferedSecs;
        this.recordingSecs = recordingSecs;
        // A save or a stop ends with its clip result, a start with the state.
        if (requestId != 0 && requestId == pendingId && pendingOp == OP_START) {
            pendingOp = 0;
            if (state != RECORDING) toast.accept(reasonText(context, reason));
        }
        update();
    }

    /** The host's result for a saved clip or a stopped recording; shows it as a toast. UI thread. */
    public void onClip(int requestId, int outcome, int reason, int durationSecs, long fileBytes) {
        int op = requestId != 0 && requestId == pendingId ? pendingOp : 0;
        if (op != 0) pendingOp = 0;
        toast.accept(clipText(context, op, outcome, reason, durationSecs, fileBytes));
        update();
    }

    private boolean waiting(int op, long now) {
        return pendingOp == op && now - pendingAt < ANSWER_MS;
    }

    Row[] rows() {
        long now = clock.getAsLong();
        boolean recording = known && state == RECORDING;
        String saveDetail = null;
        boolean saveOn = true, recordOn = true, problem = false;
        if (known) {
            switch (state) {
                case ARMED: case RECORDING:
                    saveDetail = bufferedSecs <= 0 ? reasonText(context, NO_KEYFRAME_YET)
                            : context.getString(R.string.stream_dvr_buffered, bufferedSecs);
                    break;
                case SAVING:
                    saveDetail = context.getString(R.string.stream_dvr_saving);
                    saveOn = false;
                    break;
                default:
                    // Off, unsupported, an error, or a state a newer host added: greyed out with the reason.
                    saveDetail = reasonText(context, reason);
                    problem = true;
                    saveOn = recordOn = false;
            }
        }
        if (waiting(OP_SAVE, now)) {
            saveDetail = context.getString(R.string.stream_dvr_saving);
            saveOn = false;
            problem = false;
        }
        if (waiting(OP_START, now) || waiting(OP_STOP, now)) recordOn = false;
        Row save = new Row(OP_SAVE, context.getString(R.string.stream_dvr_save), saveDetail, saveOn, problem);
        Row record = recording
                ? new Row(OP_STOP, context.getString(R.string.stream_dvr_record_stop),
                        context.getString(R.string.stream_dvr_recording, clock(recordingSecs)), recordOn, false)
                : new Row(OP_START, context.getString(R.string.stream_dvr_record_start), null, recordOn, false);
        return new Row[] {save, record};
    }

    /** Draws the group on the menu's first page; it rewrites itself as the host's answers come. */
    public void render(ActionSheet sheet) {
        this.sheet = sheet;
        group = sheet.addGroup();
        fill();
    }

    private void update() {
        if (group != null && sheet != null && sheet.isShowing()) fill();
    }

    private void fill() {
        // The row that has the focus keeps it through the rewrite.
        int focused = -1, at = 0;
        View focus = group.findFocus();
        for (int i = 0; i < group.getChildCount(); i++) {
            if (!(group.getChildAt(i) instanceof MaterialButton)) continue;
            if (group.getChildAt(i) == focus) focused = at;
            at++;
        }
        group.removeAllViews();
        sheet.addSection(group, R.string.stream_dvr_title);
        at = 0;
        for (Row row : rows()) {
            int icon = row.op == OP_SAVE ? R.drawable.ic_download : row.op == OP_STOP ? R.drawable.ic_stop : R.drawable.ic_video;
            MaterialButton button = sheet.addAction(group, icon, row.label);
            button.setEnabled(row.enabled);
            button.setOnClickListener(view -> request(row.op));
            if (row.detail != null) sheet.addNote(group, row.detail, row.problem);
            if (at++ == focused) button.requestFocus();
        }
    }

    /** The toast for a clip result: what was saved, or the host's reason. */
    static String clipText(Context context, int op, int outcome, int reason, int durationSecs, long fileBytes) {
        if (outcome != SAVED) return reasonText(context, reason);
        return op == OP_STOP
                ? context.getString(R.string.stream_dvr_recording_saved, clock(durationSecs), size(fileBytes))
                : context.getString(R.string.stream_dvr_clip_saved, durationSecs, size(fileBytes));
    }

    /** The plain-language reason the host gave for a DVR that is off or a clip it did not save. */
    static String reasonText(Context context, int reason) {
        switch (reason) {
            case DISABLED: return context.getString(R.string.stream_dvr_reason_disabled);
            case PYROWAVE: return context.getString(R.string.stream_dvr_reason_pyrowave);
            case DISK_FULL: case QUOTA_EXCEEDED: return context.getString(R.string.stream_dvr_reason_disk);
            case NO_KEYFRAME_YET: return context.getString(R.string.stream_dvr_reason_no_keyframe);
            case AUDIO_CODEC_UNSUPPORTED: return context.getString(R.string.stream_dvr_reason_audio);
            case BUSY: return context.getString(R.string.stream_dvr_reason_busy);
            case PERMISSION_DENIED: return context.getString(R.string.stream_dvr_reason_permission);
            case IO_ERROR: return context.getString(R.string.stream_dvr_reason_io);
            case NOT_STREAMING: return context.getString(R.string.stream_dvr_reason_not_streaming);
            default: return context.getString(R.string.stream_dvr_reason_unknown);
        }
    }

    /** A recording's time: 02:14, or 1:02:14 past an hour. */
    static String clock(int secs) {
        int s = Math.max(0, secs);
        if (s >= 3600) return String.format(Locale.ROOT, "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60);
        return String.format(Locale.ROOT, "%02d:%02d", s / 60, s % 60);
    }

    /** A clip's size in decimal units: 84 MB, 1.2 GB. */
    static String size(long bytes) {
        final long mb = 1_000_000L;
        if (bytes >= 1000 * mb) return String.format(Locale.getDefault(), "%.1f GB", bytes / (double) (1000 * mb));
        return Math.max(1, (bytes + mb - 1) / mb) + " MB";
    }
}
