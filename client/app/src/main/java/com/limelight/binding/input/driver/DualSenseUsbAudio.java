package com.limelight.binding.input.driver;

import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbInterface;
import android.hardware.usb.UsbManager;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;

/** Owns a separate USB connection so cancellation cannot disturb the HID input reader. */
final class DualSenseUsbAudio implements AutoCloseable {
    private final UsbManager manager;
    private final UsbDevice device;
    private final UsbDeviceConnection connection;
    private UsbInterface control, stream, idle;
    private boolean controlClaimed, streamClaimed;
    private long handle;
    private int featureIndex;
    private byte[] savedMute, savedVolume, savedMicMute, savedMicVolume;
    private int micFeatureIndex;

    private DualSenseUsbAudio(UsbManager manager, UsbDevice device, UsbDeviceConnection connection) {
        this.manager = manager;
        this.device = device;
        this.connection = connection;
    }

    static DualSenseUsbAudio open(UsbManager manager, UsbDevice device) {
        UsbDeviceConnection connection = manager.openDevice(device);
        if (connection == null) return unavailable("opening audio connection");
        DualSenseUsbAudio audio = new DualSenseUsbAudio(manager, device, connection);
        boolean ready = false;
        try {
            DualSenseAudioDescriptor descriptor = DualSenseAudioDescriptor.find(connection.getRawDescriptors());
            if (descriptor == null) return unavailable("matching four-channel audio descriptors");
            for (int i = 0; i < device.getInterfaceCount(); i++) {
                UsbInterface iface = device.getInterface(i);
                if (iface.getId() == descriptor.controlInterface) audio.control = iface;
                if (iface.getId() == descriptor.streamingInterface) {
                    if (iface.getAlternateSetting() == descriptor.alternateSetting) audio.stream = iface;
                    if (iface.getAlternateSetting() == 0) audio.idle = iface;
                }
            }
            if (audio.control == null || audio.stream == null || audio.idle == null) {
                return unavailable("finding audio interfaces");
            }
            audio.controlClaimed = connection.claimInterface(audio.control, true);
            if (!audio.controlClaimed) return unavailable("claiming audio control interface");
            audio.streamClaimed = connection.claimInterface(audio.stream, true);
            if (!audio.streamClaimed) return unavailable("claiming audio streaming interface");
            if (!connection.setInterface(audio.stream)) return unavailable("selecting audio streaming alternate");
            audio.featureIndex = (descriptor.featureUnit << 8) | descriptor.controlInterface;
            byte[] mute = new byte[1], volume = new byte[2];
            if (audio.audioControl(true, 1, mute) != mute.length ||
                    audio.audioControl(true, 2, volume) != volume.length) return unavailable("reading audio mute and volume");
            audio.savedMute = mute;
            audio.savedVolume = volume;
            // USB Audio master mute and volume affect the actuator channels too.
            // Use unmuted 0 dB, then restore the prior values before returning audio to Android.
            if (audio.audioControl(false, 1, new byte[1]) != 1 ||
                    audio.audioControl(false, 2, new byte[2]) != 2) return unavailable("setting audio mute and volume");
            audio.handle = MoonBridge.openUsbHaptics(connection.getFileDescriptor(),
                    descriptor.endpoint, descriptor.interval);
            if (audio.handle == 0) return unavailable("opening isochronous audio transport");
            // Probe real isochronous output with silence before advertising the capability.
            ready = audio.write(new byte[DualSenseHapticsQueue.USB_BYTES]);
            return ready ? audio : unavailable("probing isochronous audio transfer");
        }
        finally {
            if (!ready) audio.close();
        }
    }

    private static DualSenseUsbAudio unavailable(String stage) {
        LimeLog.warning("DualSense USB waveform initialization failed while " + stage);
        return null;
    }

    DualSenseUsbMicrophone openMicrophone() {
        DualSenseAudioDescriptor descriptor = DualSenseAudioDescriptor.findMicrophone(connection.getRawDescriptors());
        if (descriptor == null) return null;
        micFeatureIndex = (descriptor.featureUnit << 8) | descriptor.controlInterface;
        if (savedMicMute == null) {
            byte[] mute = new byte[1], volume = new byte[2];
            if (connection.controlTransfer(0xa1, 0x81, 0x100, micFeatureIndex, mute, 1, 100) != 1 ||
                    connection.controlTransfer(0xa1, 0x81, 0x200, micFeatureIndex, volume, 2, 100) != 2) return null;
            savedMicMute = mute; savedMicVolume = volume;
        }
        if (connection.controlTransfer(0x21, 1, 0x100, micFeatureIndex, new byte[1], 1, 100) != 1 ||
                connection.controlTransfer(0x21, 1, 0x200, micFeatureIndex, new byte[2], 2, 100) != 2) return null;
        return DualSenseUsbMicrophone.open(manager, device);
    }

    boolean write(byte[] samples) {
        return MoonBridge.writeUsbHaptics(handle, samples);
    }

    private int audioControl(boolean read, int selector, byte[] data) {
        return connection.controlTransfer(read ? 0xa1 : 0x21, read ? 0x81 : 0x01,
                selector << 8, featureIndex, data, data.length, 100);
    }

    @Override
    public void close() {
        if (streamClaimed) {
            connection.setInterface(idle);
            if (savedMute != null) audioControl(false, 1, savedMute);
            if (savedVolume != null) audioControl(false, 2, savedVolume);
            connection.releaseInterface(stream);
        }
        if (controlClaimed) {
            if (savedMicMute != null) connection.controlTransfer(0x21, 1, 0x100, micFeatureIndex, savedMicMute, 1, 100);
            if (savedMicVolume != null) connection.controlTransfer(0x21, 1, 0x200, micFeatureIndex, savedMicVolume, 2, 100);
            connection.releaseInterface(control);
        }
        connection.close();
        // This closes the final descriptor, cancelling all URBs before their buffers are freed.
        if (handle != 0) {
            MoonBridge.closeUsbHaptics(handle);
            handle = 0;
        }
        if (controlClaimed) {
            UsbDeviceConnection restore = null;
            try {
                restore = manager.openDevice(device);
                if (restore != null) MoonBridge.restoreUsbAudio(restore.getFileDescriptor(), control.getId());
            }
            catch (SecurityException e) {
                LimeLog.warning("Reconnect DualSense to restore Android USB audio");
            }
            finally {
                if (restore != null) restore.close();
            }
            controlClaimed = streamClaimed = false;
        }
    }
}
