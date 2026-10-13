package com.limelight.binding.input.driver;

public abstract class AbstractController {

    private final int deviceId;
    private final int vendorId;
    private final int productId;

    protected final UsbDriverListener listener;

    protected int buttonFlags, supportedButtonFlags;
    protected float leftTrigger, rightTrigger;
    protected float rightStickX, rightStickY;
    protected float leftStickX, leftStickY;
    protected short capabilities;
    protected byte type;

    public int getControllerId() {
        return deviceId;
    }

    public int getVendorId() {
        return vendorId;
    }

    public int getProductId() {
        return productId;
    }

    public int getSupportedButtonFlags() {
        return supportedButtonFlags;
    }

    public short getCapabilities() {
        return capabilities;
    }

    public byte getType() {
        return type;
    }

    protected void setButtonFlag(int buttonFlag, int data) {
        if (data != 0) {
            buttonFlags |= buttonFlag;
        }
        else {
            buttonFlags &= ~buttonFlag;
        }
    }

    protected void reportInput() {
        listener.reportControllerState(deviceId, buttonFlags, leftStickX, leftStickY,
                rightStickX, rightStickY, leftTrigger, rightTrigger);
    }

    public abstract boolean start();
    public abstract void stop();

    public AbstractController(int deviceId, UsbDriverListener listener, int vendorId, int productId) {
        this.deviceId = deviceId;
        this.listener = listener;
        this.vendorId = vendorId;
        this.productId = productId;
    }

    public abstract void rumble(short lowFreqMotor, short highFreqMotor);

    public abstract void rumbleTriggers(short leftTrigger, short rightTrigger);

    public void controllerHaptics(int sequence, byte[] pcm) {}

    public void setAdaptiveTriggers(byte eventFlags, byte typeLeft, byte typeRight, byte[] left, byte[] right) {}

    public void setLed(byte r, byte g, byte b) {}

    public void setPlayerNumber(int playerNumber) {}

    public void setMotionEventState(byte motionType, short reportRateHz) {}

    protected void notifyDeviceRemoved() {
        listener.deviceRemoved(this);
    }

    protected void notifyDeviceAdded() {
        listener.deviceAdded(this);
    }
}
