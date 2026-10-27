package com.limelight.files;

import androidx.annotation.StringRes;

import com.limelight.R;
import com.limelight.nvstream.http.HostCapabilities;

/**
 * What the screen says about file transfer: the shared problem ids (nightjar-core's {@code Problem::id}) and
 * whether this device may send or download on a PC. The host grants file transfer per device, never through
 * Full control, so a missing permission always points to the host's web UI.
 */
public final class FileProblems {
    public static final String NO_PERMISSION = "no_permission";
    public static final String TRANSFER_OFF = "transfer_off";
    public static final String INVALID_NAME = "invalid_name";
    public static final String INTERRUPTED = "interrupted";
    public static final String TOO_LARGE = "too_large";
    public static final String NO_SPACE = "no_space";
    public static final String NO_CONSOLE_USER = "no_console_user";
    public static final String BUSY = "busy";
    public static final String NOT_FOUND = "not_found";
    public static final String NETWORK = "network";
    public static final String LOCAL = "local";
    public static final String HOST = "host";
    public static final String CANCELLED = "cancelled";

    private FileProblems() {}

    /** The text for a problem id; an id this build does not know reads as the host's error. */
    @StringRes
    public static int text(String problem) {
        if (problem == null) return R.string.files_problem_host;
        switch (problem) {
            case NO_PERMISSION: return R.string.files_problem_no_permission;
            case TRANSFER_OFF: return R.string.files_problem_transfer_off;
            case INVALID_NAME: return R.string.files_problem_invalid_name;
            case INTERRUPTED: return R.string.files_problem_interrupted;
            case TOO_LARGE: return R.string.files_problem_too_large;
            case NO_SPACE: return R.string.files_problem_no_space;
            case NO_CONSOLE_USER: return R.string.files_problem_no_console_user;
            case BUSY: return R.string.files_problem_busy;
            case NOT_FOUND: return R.string.files_problem_not_found;
            case NETWORK: return R.string.files_problem_network;
            case LOCAL: return R.string.files_problem_local;
            case CANCELLED: return R.string.files_cancelled;
            default: return R.string.files_problem_host;
        }
    }

    /** Whether this device may use one direction on a PC. */
    public enum Access { READY, CHECKING, NO_PERMISSION, OFF, UNAVAILABLE }

    /**
     * From the PC's capabilities after {@link HostCapabilities#withNativeCapabilities}: unknown until the
     * native API answered, off when the host does not offer it (its File transfer setting), no permission when
     * it does but not to this device.
     */
    public static Access access(HostCapabilities caps, boolean upload) {
        if (!caps.availability(HostCapabilities.Feature.NATIVE_API).isSupported()) {
            return Access.UNAVAILABLE;
        }
        HostCapabilities.Availability availability = caps.availability(
                upload ? HostCapabilities.Feature.FILE_UPLOAD : HostCapabilities.Feature.FILE_DOWNLOAD);
        if (availability.isSupported()) return Access.READY;
        if (availability.isUnknown()) return Access.CHECKING;
        if (availability.reason == HostCapabilities.Reason.FILE_PERMISSION) return Access.NO_PERMISSION;
        return Access.OFF;
    }

    /** The note for a direction that is not ready, or 0 when it is. */
    @StringRes
    public static int accessText(Access access, boolean upload) {
        switch (access) {
            case CHECKING: return R.string.files_checking;
            case NO_PERMISSION: return upload ? R.string.files_no_upload_permission :
                    R.string.files_no_download_permission;
            case OFF: return R.string.files_off;
            case UNAVAILABLE: return R.string.files_unavailable;
            default: return 0;
        }
    }
}
