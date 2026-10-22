package com.limelight.nvstream.jni;

/**
 * The Rubylight native API (protocol/android, native_api.rs) over HTTPS from the Rust client.
 * Every call blocks on the network, so callers keep it off the UI thread. It never throws for
 * a failed request: the answer is always a JSON envelope, {"ok": ...} or {"error": {...}}.
 */
public final class NativeApiBridge {
    private NativeApiBridge() {}

    /** Whether libnightjar_core.so is loaded; false on a device or build without the Rust library. */
    public static boolean isAvailable() {
        return RustBridge.isAvailable();
    }

    /**
     * @param op capabilities, library, session, session_action, radeon, radeon_write or radeon_restore
     * @param httpsPort 0 to let the client ask the host's serverinfo
     * @param serverCertificatePem the pinned certificate of the paired host
     * @param clientCertificateDer the client certificate, DER
     * @param clientPrivateKeyPkcs8 the client key, PKCS#8 DER
     * @param argument the session action (disconnect, end, close_game) or the radeon_write JSON; "" otherwise
     */
    public static native String request(String op, String address, int httpPort, int httpsPort,
                                        String serverCertificatePem, String uniqueId,
                                        byte[] clientCertificateDer, byte[] clientPrivateKeyPkcs8,
                                        String argument);
}
