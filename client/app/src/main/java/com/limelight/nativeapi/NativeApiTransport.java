package com.limelight.nativeapi;

/** Carries one native API call to a host; JNI in the app, a fake in the tests. Blocking. */
public interface NativeApiTransport {
    /** Returns the JSON envelope {"ok": ...} or {"error": {...}}; throws only when there is no answer. */
    String request(HostEndpoint host, String op, String argument) throws NativeApiException;
}
