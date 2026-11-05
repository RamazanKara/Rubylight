package com.limelight.nativeapi;

/** A {@link StreamMove} as a host says it. */
public final class StreamMoveFixtures {
    private StreamMoveFixtures() {
    }

    public static StreamMove playing(String device, int appId) {
        return new StreamMove(device, appId, "", "Game", null, false, false);
    }
}
