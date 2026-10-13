package com.limelight.binding.input.driver;

import android.hardware.usb.*;
import com.limelight.nvstream.jni.MoonBridge;

/** Separate open file description: USB completion queues must not mix input and output URBs. */
final class DualSenseUsbMicrophone implements AutoCloseable {
    private final UsbDeviceConnection connection;
    private UsbInterface stream, idle;
    private boolean claimed;
    private long handle;

    private DualSenseUsbMicrophone(UsbDeviceConnection connection) { this.connection = connection; }

    static DualSenseUsbMicrophone open(UsbManager manager, UsbDevice device) {
        UsbDeviceConnection connection = manager.openDevice(device);
        if (connection == null) return null;
        DualSenseUsbMicrophone mic = new DualSenseUsbMicrophone(connection);
        boolean ready = false;
        try {
            DualSenseAudioDescriptor descriptor = DualSenseAudioDescriptor.findMicrophone(connection.getRawDescriptors());
            if (descriptor == null) return null;
            for (int i = 0; i < device.getInterfaceCount(); i++) {
                UsbInterface iface = device.getInterface(i);
                if (iface.getId() == descriptor.streamingInterface) {
                    if (iface.getAlternateSetting() == descriptor.alternateSetting) mic.stream = iface;
                    if (iface.getAlternateSetting() == 0) mic.idle = iface;
                }
            }
            if (mic.stream == null || mic.idle == null) return null;
            mic.claimed = connection.claimInterface(mic.stream, true);
            if (!mic.claimed || !connection.setInterface(mic.stream)) return null;
            mic.handle = MoonBridge.openUsbMicrophone(connection.getFileDescriptor(), descriptor.endpoint, descriptor.interval);
            ready = mic.handle != 0;
            return ready ? mic : null;
        } finally { if (!ready) mic.close(); }
    }

    int read(byte[] pcm) { return MoonBridge.readUsbMicrophone(handle, pcm); }
    @Override public void close() {
        if (claimed) { connection.setInterface(idle); connection.releaseInterface(stream); }
        connection.close();
        if (handle != 0) { MoonBridge.closeUsbHaptics(handle); handle = 0; }
    }
}
