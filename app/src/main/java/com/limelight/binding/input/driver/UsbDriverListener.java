package com.limelight.binding.input.driver;

public interface UsbDriverListener {
    void reportControllerState(int controllerId, int buttonFlags,
                               float leftStickX, float leftStickY,
                               float rightStickX, float rightStickY,
                               float leftTrigger, float rightTrigger);

    void reportControllerTouch(int controllerId, byte eventType, int pointerId, float x, float y, float pressure);
    void reportControllerMotion(int controllerId, byte motionType, float x, float y, float z);
    void reportControllerBattery(int controllerId, byte state, byte percentage);

    void deviceRemoved(AbstractController controller);
    void deviceAdded(AbstractController controller);

    // The driver for this attached USB device could not start, so it will not be handed over.
    default void deviceStartFailed(int usbDeviceId) {}
}
