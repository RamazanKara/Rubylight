package com.limelight.nativeapi;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.NativeApiBridge;

/** The Rust client (libnightjar_core.so) behind {@link NativeApiTransport}. */
public final class JniNativeApiTransport implements NativeApiTransport {
    @Override
    public String request(HostEndpoint host, String op, String argument) throws NativeApiException {
        if (!NativeApiBridge.isAvailable()) {
            throw new NativeApiException(NativeApiException.Kind.UNAVAILABLE, "The Rust library is not loaded");
        }
        String reply;
        try {
            reply = NativeApiBridge.request(op, host.address, host.httpPort, host.httpsPort,
                    host.serverCertificatePem, host.uniqueId, host.clientCertificateDer,
                    host.clientPrivateKeyPkcs8, argument == null ? "" : argument);
        } catch (UnsatisfiedLinkError e) {
            LimeLog.warning("Native API unavailable: " + e.getMessage());
            throw new NativeApiException(NativeApiException.Kind.UNAVAILABLE, "The Rust library lacks the native API");
        }
        if (reply == null) {
            throw new NativeApiException(NativeApiException.Kind.MALFORMED, "No answer from the native client");
        }
        return reply;
    }
}
