package com.limelight.nvstream;

import com.limelight.nvstream.http.LaunchConfirmation;

public interface NvConnectionListener {
    void stageStarting(String stage);
    void stageComplete(String stage);
    void stageFailed(String stage, int portFlags, int errorCode);
    
    void connectionStarted();
    void connectionTerminated(int errorCode);
    void connectionStatusUpdate(int connectionStatus);
    
    void displayMessage(String message);
    void displayTransientMessage(String message);

    default void launchConfirmationRequired(LaunchConfirmation confirmation) { confirmation.cancel(); }
    default void launchConfirmationFinished() {}
    default void launchActionCompleted(String message) {}

    void rumble(short controllerNumber, short lowFreqMotor, short highFreqMotor);
    void rumbleTriggers(short controllerNumber, short leftTrigger, short rightTrigger);

    void setHdrMode(boolean enabled, byte[] hdrMetadata);

    void setMotionEventState(short controllerNumber, byte motionType, short reportRateHz);

    void setControllerLED(short controllerNumber, byte r, byte g, byte b);

    default void controllerAudio(int kind, byte[] payload) {}
    default void controllerHaptics(short controllerNumber, int sequence, byte[] pcm) {}
    default void cursorShape(byte[] message) {}

    /**
     * The host's notice that it holds the video bitrate below the requested one because of the
     * network (state 1 limited, 2 floor), or is back at it (state 0). Bitrates in kbps.
     */
    default void linkNotice(int state, int ceilingKbps, int currentKbps) {}

    default void setAdaptiveTriggers(short controllerNumber, byte eventFlags, byte typeLeft,
                                     byte typeRight, byte[] left, byte[] right) {}

    /**
     * A Steam Controller haptic output report (id first, 1 to 12 bytes) for the real controller's
     * pads. No driver owns a Steam Controller yet, so nothing advertises the capability.
     */
    default void steamHaptic(short controllerNumber, byte[] report) {}

    /**
     * A Steam Controller settings write: command 0x87 with (register, lo, hi) triples back to
     * back, or 0x8E (load defaults) with none. Unused, like {@link #steamHaptic}.
     */
    default void steamSettings(short controllerNumber, byte command, byte[] triples) {}
}
