package com.limelight.nativeapi;

/** A native API call that did not produce an answer: the transport failed or the host refused. */
public class NativeApiException extends Exception {
    /**
     * The kinds of native_api.rs, plus UNAVAILABLE when the Rust library is not loaded. POWER_REFUSED is a
     * documented Sleep PC refusal: the message is the host's status_message as sent, possibly empty.
     */
    public enum Kind {
        NETWORK, TLS, CERTIFICATE_MISMATCH, STATUS, NOT_FOUND, API, MALFORMED, NOT_PAIRED,
        IDENTITY, INVALID_ARGUMENT, CANCELLED, POWER_REFUSED, UNAVAILABLE;

        static Kind of(String wire) {
            for (Kind kind : values()) {
                if (kind.name().equalsIgnoreCase(wire)) {
                    return kind;
                }
            }
            return MALFORMED;
        }
    }

    private final Kind kind;
    private final int httpStatus;
    private final String code;
    private final String setting;
    private final long retryAfterMs;

    public NativeApiException(Kind kind, String message, int httpStatus, String code, String setting,
                              long retryAfterMs) {
        super(message);
        this.kind = kind;
        this.httpStatus = httpStatus;
        this.code = code;
        this.setting = setting;
        this.retryAfterMs = retryAfterMs;
    }

    public NativeApiException(Kind kind, String message) {
        this(kind, message, 0, null, null, 0);
    }

    public Kind getKind() {
        return kind;
    }

    /** The HTTP status of the host's answer, or 0 when there was none. */
    public int getHttpStatus() {
        return httpStatus;
    }

    /** The API's error code (not_session_owner, host_control_required, ...) or null. */
    public String getCode() {
        return code;
    }

    /** The setting the error is about, or null. */
    public String getSetting() {
        return setting;
    }

    /** How long the host asks to wait before retrying, 0 when it does not. */
    public long getRetryAfterMs() {
        return retryAfterMs;
    }

    /** Whether the host could not be reached at all, as opposed to answering with a refusal. */
    public boolean isUnreachable() {
        return kind == Kind.NETWORK || kind == Kind.TLS || kind == Kind.UNAVAILABLE;
    }
}
