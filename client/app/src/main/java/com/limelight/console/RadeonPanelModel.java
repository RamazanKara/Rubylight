package com.limelight.console;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeRadeon;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;
import java.util.Objects;

/**
 * What the Radeon panel of the stream menu draws for one GET /radeon answer. The host sends the five
 * states of docs/native-api.md ("Radeon state and writes"); this class decides, per setting and for the
 * panel, what a client shows for each, and never offers a control that would do nothing:
 * <ul>
 * <li>configurable: an editable control, only while the host says {@code writable_now};</li>
 * <li>read only: the value;</li>
 * <li>needs external setup: the value if there is one, and the host's message;</li>
 * <li>unsupported: greyed, "not supported on this GPU or driver";</li>
 * <li>temporarily unavailable: greyed, the host's message; the panel reads again later.</li>
 * </ul>
 * Setting ids the app has no name for are drawn from their kind, range, options and unit.
 */
public final class RadeonPanelModel {
    /** What the panel as a whole says above its settings. */
    public enum Notice {
        NONE, READ_ONLY, UNSUPPORTED, UNAVAILABLE, ADLX_UNAVAILABLE, NEEDS_EXTERNAL_SETUP, NEEDS_HOST_CONTROL, NEEDS_SESSION
    }

    /** How one setting is drawn. */
    public enum Mode { EDIT, VALUE, EXTERNAL_SETUP, UNSUPPORTED, UNAVAILABLE }

    public static final class Row {
        public final int gpu;
        public final String id;
        public final NativeRadeon.Kind kind;
        public final Mode mode;
        /** Boolean, Long or String; null when the host reports none. */
        public final Object value;
        public final NativeRadeon.Range range;
        public final List<NativeRadeon.Choice> options;
        public final String unit;
        /** The host's text for the user, or null. */
        public final String message;
        /** Pending change: {@code original} comes back when the session ends. Null when nothing is pending. */
        public final NativeRadeon.Restore restore;
        /** Name of the setting's group (anti_lag, chill, ...) and of its field (enabled, max_fps, ...). */
        public final String group;
        public final String field;

        Row(int gpu, String id, String group, NativeRadeon.Kind kind, Mode mode, Object value, NativeRadeon.Range range,
            List<NativeRadeon.Choice> options, String unit, String message, NativeRadeon.Restore restore) {
            this.gpu = gpu;
            this.id = id;
            int dot = id.indexOf('.');
            this.group = group != null && !group.isEmpty() ? group : dot < 0 ? id : id.substring(0, dot);
            this.field = dot < 0 ? "" : id.substring(dot + 1);
            this.kind = kind;
            this.mode = mode;
            this.value = value;
            this.range = range;
            this.options = options;
            this.unit = unit;
            this.message = message;
            this.restore = restore;
        }

        /** The title's string resource when the app knows the group, else 0. */
        public int groupTitleRes() {
            return groupRes(group);
        }

        /** The field's string resource when it is not the plain on/off of the group, else 0. */
        public int fieldTitleRes() {
            return fieldRes(field);
        }

        /** Title for an id the app has no name for: its words, capitalised ("chill.max_fps" is "Chill: Max fps"). */
        public String genericTitle() {
            String groupText = words(group);
            String fieldText = words(field);
            return fieldText.isEmpty() || "Enabled".equals(fieldText) ? groupText : groupText + ": " + fieldText;
        }

        /** The setting's name: the app's own for a known group and field, else made from the id's words. */
        public String title(android.content.res.Resources resources) {
            int groupRes = groupTitleRes();
            if (groupRes == 0) {
                return genericTitle();
            }
            String groupText = resources.getString(groupRes);
            int fieldRes = fieldTitleRes();
            if (fieldRes != 0) {
                return resources.getString(R.string.native_title_field, groupText, resources.getString(fieldRes));
            }
            String fieldText = words(field);
            boolean plain = fieldText.isEmpty() || "Enabled".equals(fieldText) || "Mode".equals(fieldText);
            return plain ? groupText : resources.getString(R.string.native_title_field, groupText, fieldText);
        }

        /** The value as text for an integer or a choice; null for a toggle. */
        public String valueText() {
            if (value == null) {
                return null;
            }
            if (kind == NativeRadeon.Kind.TOGGLE || value instanceof Boolean) {
                return null;
            }
            String text = String.valueOf(value);
            if (value instanceof String) {
                for (NativeRadeon.Choice choice : options) {
                    if (choice.value.equals(value)) {
                        text = choice.label == null || choice.label.isEmpty() ? choice.value : choice.label;
                    }
                }
            }
            return withUnit(text, unit);
        }

        @Override
        public boolean equals(Object other) {
            if (!(other instanceof Row)) {
                return false;
            }
            Row row = (Row) other;
            return gpu == row.gpu && id.equals(row.id) && kind == row.kind && mode == row.mode
                    && Objects.equals(value, row.value) && Objects.equals(unit, row.unit)
                    && Objects.equals(message, row.message) && sameRange(range, row.range)
                    && sameChoices(options, row.options) && sameRestore(restore, row.restore);
        }

        @Override
        public int hashCode() {
            return id.hashCode() * 31 + Objects.hashCode(value);
        }
    }

    public static final class Gpu {
        public final int id;
        public final String name;
        public final String driver;
        public final List<Row> rows;

        Gpu(int id, String name, String driver, List<Row> rows) {
            this.id = id;
            this.name = name;
            this.driver = driver;
            this.rows = Collections.unmodifiableList(rows);
        }
    }

    public final long revision;
    public final Notice notice;
    /** The host's text for the panel notice, or null. */
    public final String message;
    public final List<Gpu> gpus;

    private RadeonPanelModel(long revision, Notice notice, String message, List<Gpu> gpus) {
        this.revision = revision;
        this.notice = notice;
        this.message = message;
        this.gpus = Collections.unmodifiableList(gpus);
    }

    public static RadeonPanelModel of(NativeRadeon radeon) {
        NativeRadeon.ControlState state = radeon.state;
        Notice notice;
        switch (state) {
            case CONFIGURABLE:
                notice = radeon.writableNow ? Notice.NONE
                        : radeon.hostControl ? Notice.NEEDS_SESSION : Notice.NEEDS_HOST_CONTROL;
                break;
            case READ_ONLY:
                notice = Notice.READ_ONLY;
                break;
            case NEEDS_EXTERNAL_SETUP:
                notice = Notice.NEEDS_EXTERNAL_SETUP;
                break;
            case TEMPORARILY_UNAVAILABLE:
                notice = "adlx_unavailable".equals(radeon.reason) ? Notice.ADLX_UNAVAILABLE : Notice.UNAVAILABLE;
                break;
            default:
                // Unsupported, and a state this app does not know: both draw as not supported.
                notice = Notice.UNSUPPORTED;
                break;
        }
        boolean editable = state == NativeRadeon.ControlState.CONFIGURABLE && radeon.writableNow;
        List<Gpu> gpus = new ArrayList<>();
        for (NativeRadeon.Gpu gpu : radeon.gpus) {
            List<Row> rows = new ArrayList<>();
            for (NativeRadeon.Setting setting : gpu.settings) {
                rows.add(row(gpu.id, setting, editable));
            }
            gpus.add(new Gpu(gpu.id, gpu.name, gpu.driver, rows));
        }
        return new RadeonPanelModel(radeon.revision, notice, radeon.message, gpus);
    }

    private static Row row(int gpu, NativeRadeon.Setting setting, boolean editable) {
        Mode mode;
        switch (setting.state) {
            case CONFIGURABLE:
                mode = editable && controllable(setting) ? Mode.EDIT : Mode.VALUE;
                break;
            case READ_ONLY:
                mode = Mode.VALUE;
                break;
            case NEEDS_EXTERNAL_SETUP:
                mode = Mode.EXTERNAL_SETUP;
                break;
            case TEMPORARILY_UNAVAILABLE:
                mode = Mode.UNAVAILABLE;
                break;
            default:
                mode = Mode.UNSUPPORTED;
                break;
        }
        return new Row(gpu, setting.id, setting.group, setting.kind, mode, setting.value, setting.range, setting.options,
                setting.unit, setting.message, setting.restore);
    }

    // A control needs a value to start from, and a slider needs a range with room to move.
    private static boolean controllable(NativeRadeon.Setting setting) {
        if (setting.kind == NativeRadeon.Kind.UNKNOWN || setting.value == null) {
            return false;
        }
        if (setting.kind == NativeRadeon.Kind.INTEGER) {
            return setting.value instanceof Number && setting.range != null && setting.range.max > setting.range.min;
        }
        if (setting.kind == NativeRadeon.Kind.CHOICE) {
            return !setting.options.isEmpty();
        }
        return setting.value instanceof Boolean;
    }

    /** Whether the panel has anything to draw below its notice. */
    public boolean hasRows() {
        for (Gpu gpu : gpus) {
            if (!gpu.rows.isEmpty()) {
                return true;
            }
        }
        return false;
    }

    public Row find(String id) {
        for (Gpu gpu : gpus) {
            for (Row row : gpu.rows) {
                if (row.id.equals(id)) {
                    return row;
                }
            }
        }
        return null;
    }

    /** True when drawing {@code other} would show exactly what this shows. */
    public boolean sameAs(RadeonPanelModel other) {
        if (other == null || notice != other.notice || !Objects.equals(message, other.message)
                || gpus.size() != other.gpus.size()) {
            return false;
        }
        for (int i = 0; i < gpus.size(); i++) {
            Gpu a = gpus.get(i);
            Gpu b = other.gpus.get(i);
            if (a.id != b.id || !Objects.equals(a.name, b.name) || !Objects.equals(a.driver, b.driver)
                    || !a.rows.equals(b.rows)) {
                return false;
            }
        }
        return true;
    }

    /** What a write sends: the row's value moved to {@code requested}, snapped into the range and its step. */
    public static Object writableValue(Row row, Object requested) {
        if (row.kind == NativeRadeon.Kind.INTEGER && requested instanceof Number) {
            long value = ((Number) requested).longValue();
            NativeRadeon.Range range = row.range;
            if (range != null) {
                long step = Math.max(1, range.step);
                value = range.min + Math.round((double) (value - range.min) / step) * step;
                value = Math.max(range.min, Math.min(range.max, value));
            }
            return value;
        }
        return requested;
    }

    /** Why a write or a read failed, as the panel shows it next to the setting. */
    public static final class Failure {
        /** Message string resource, always set. */
        public final int messageRes;
        /** The setting the host named, or null when the failure is about the whole request. */
        public final String settingId;
        /** The host's own text, shown when the app has no better one; may be null. */
        public final String hostMessage;

        Failure(int messageRes, String settingId, String hostMessage) {
            this.messageRes = messageRes;
            this.settingId = settingId;
            this.hostMessage = hostMessage;
        }

        /** The failure as the panel shows it: the host's code decides, not its English. */
        public static Failure of(NativeApiException e, String requestedSetting) {
            String code = e.getCode() == null ? "" : e.getCode();
            String setting = e.getSetting() != null ? e.getSetting() : requestedSetting;
            int message;
            switch (code) {
                case "invalid_value":
                    message = R.string.native_error_invalid_value;
                    break;
                case "not_writable":
                    message = R.string.native_error_not_writable;
                    break;
                case "driver_rejected":
                    message = R.string.native_error_driver_rejected;
                    break;
                case "host_control_required":
                case "not_host_control":
                    message = R.string.native_error_host_control;
                    break;
                case "no_session":
                case "not_session_owner":
                    message = R.string.native_error_no_session;
                    break;
                case "session_busy":
                case "temporarily_unavailable":
                    message = R.string.native_error_busy;
                    break;
                default:
                    message = e.isUnreachable() ? R.string.native_error_unreachable
                            : e.getHttpStatus() == 503 ? R.string.native_error_busy : R.string.native_error_generic;
                    break;
            }
            return new Failure(message, setting, message == R.string.native_error_generic ? e.getMessage() : null);
        }

        @Override
        public boolean equals(Object other) {
            if (!(other instanceof Failure)) {
                return false;
            }
            Failure failure = (Failure) other;
            return messageRes == failure.messageRes && Objects.equals(settingId, failure.settingId)
                    && Objects.equals(hostMessage, failure.hostMessage);
        }

        @Override
        public int hashCode() {
            return messageRes * 31 + Objects.hashCode(settingId);
        }
    }

    /** Whether the host has no Radeon endpoints (a host from before them answers 404). */
    public static boolean isMissing(NativeApiException e) {
        return e.getKind() == NativeApiException.Kind.NOT_FOUND || e.getHttpStatus() == 404;
    }

    /** "144 fps", "70%", or just the number when there is no unit. */
    public static String withUnit(String value, String unit) {
        if (unit == null || unit.isEmpty()) {
            return value;
        }
        return "%".equals(unit) ? value + unit : value + " " + unit;
    }

    /** A pending change's original value as text: "120 fps", "On", or the option's own text. */
    public static String restoreText(Row row, NativeRadeon.Restore restore, String on, String off) {
        Object original = restore.original;
        if (original instanceof Boolean) {
            return (Boolean) original ? on : off;
        }
        String text = String.valueOf(original);
        if (original instanceof String) {
            for (NativeRadeon.Choice choice : row.options) {
                if (choice.value.equals(original) && choice.label != null && !choice.label.isEmpty()) {
                    text = choice.label;
                }
            }
        }
        return withUnit(text, row.unit);
    }

    static int groupRes(String group) {
        switch (group) {
            case "anti_lag": return R.string.native_group_anti_lag;
            case "chill": return R.string.native_group_chill;
            case "boost": return R.string.native_group_boost;
            case "image_sharpening": return R.string.native_group_image_sharpening;
            case "enhanced_sync": return R.string.native_group_enhanced_sync;
            case "frame_rate_target": return R.string.native_group_frame_rate_target;
            case "afmf": return R.string.native_group_afmf;
            case "rsr": return R.string.native_group_rsr;
            case "vsync": return R.string.native_group_vsync;
            default: return 0;
        }
    }

    static int fieldRes(String field) {
        switch (field) {
            case "min_fps": return R.string.native_field_min_fps;
            case "max_fps": return R.string.native_field_max_fps;
            case "fps": return R.string.native_field_fps;
            case "min_resolution": return R.string.native_field_min_resolution;
            case "sharpness": return R.string.native_field_sharpness;
            default: return 0;
        }
    }

    private static String words(String id) {
        StringBuilder text = new StringBuilder();
        for (String word : id.split("_")) {
            if (word.isEmpty()) {
                continue;
            }
            if (text.length() > 0) {
                text.append(' ');
                text.append(word.toLowerCase(Locale.ROOT));
            } else {
                text.append(word.substring(0, 1).toUpperCase(Locale.ROOT)).append(word.substring(1).toLowerCase(Locale.ROOT));
            }
        }
        return text.toString();
    }

    private static boolean sameRange(NativeRadeon.Range a, NativeRadeon.Range b) {
        return a == b || (a != null && b != null && a.min == b.min && a.max == b.max && a.step == b.step);
    }

    private static boolean sameChoices(List<NativeRadeon.Choice> a, List<NativeRadeon.Choice> b) {
        if (a.size() != b.size()) {
            return false;
        }
        for (int i = 0; i < a.size(); i++) {
            if (!a.get(i).value.equals(b.get(i).value) || !Objects.equals(a.get(i).label, b.get(i).label)) {
                return false;
            }
        }
        return true;
    }

    private static boolean sameRestore(NativeRadeon.Restore a, NativeRadeon.Restore b) {
        return a == b || (a != null && b != null && Objects.equals(a.original, b.original)
                && Objects.equals(a.written, b.written));
    }
}
