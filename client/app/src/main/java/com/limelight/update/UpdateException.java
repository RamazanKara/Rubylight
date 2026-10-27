package com.limelight.update;

import java.io.IOException;

/** An update that cannot continue, with a reason the dialog can explain. */
public class UpdateException extends IOException {
    public enum Reason {
        /** The release has no APK for this build. */
        NO_APK,
        /** The release has no SHA256SUMS file, so the download could not be verified. */
        NO_CHECKSUMS,
        /** The SHA256SUMS file does not list the APK. */
        CHECKSUM_ENTRY_MISSING,
        CHECKSUM_MISMATCH,
        DOWNLOAD_FAILED,
        CANCELLED
    }

    public final Reason reason;

    public UpdateException(Reason reason, Throwable cause) {
        super(reason.name(), cause);
        this.reason = reason;
    }

    public UpdateException(Reason reason) {
        this(reason, null);
    }

    /** GitHub answered 403 or 429 for an anonymous caller: wait before asking again. */
    public static final class RateLimited extends IOException {
        public RateLimited() {
            super("GitHub rate limit");
        }
    }
}
