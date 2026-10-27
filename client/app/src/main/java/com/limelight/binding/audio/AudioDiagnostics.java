package com.limelight.binding.audio;

import android.media.AudioDeviceInfo;

/** Text for the audio line of a problem report: where the sound goes and what was dropped. */
final class AudioDiagnostics {
    private AudioDiagnostics() {
    }

    /** Output device kinds only. Names and addresses of devices stay out of reports. */
    static String routes(int[] types) {
        if (types == null || types.length == 0) return "none";
        StringBuilder text = new StringBuilder();
        for (int type : types) {
            if (text.length() > 0) text.append('+');
            text.append(typeName(type));
        }
        return text.toString();
    }

    /**
     * The device the output is routed to, when known. Otherwise every attached output, labelled as
     * such, because the sound may be going to any of them.
     */
    static String route(int routedType, int[] attachedTypes) {
        if (routedType >= 0) return typeName(routedType);
        return "unknown (attached: " + routes(attachedTypes) + ")";
    }

    static String typeName(int type) {
        switch (type) {
            case AudioDeviceInfo.TYPE_BUILTIN_SPEAKER: return "speaker";
            case AudioDeviceInfo.TYPE_BUILTIN_EARPIECE: return "earpiece";
            case AudioDeviceInfo.TYPE_WIRED_HEADSET: return "wired-headset";
            case AudioDeviceInfo.TYPE_WIRED_HEADPHONES: return "wired-headphones";
            case AudioDeviceInfo.TYPE_LINE_ANALOG: return "line";
            case AudioDeviceInfo.TYPE_BLUETOOTH_SCO: return "bluetooth-sco";
            case AudioDeviceInfo.TYPE_BLUETOOTH_A2DP: return "bluetooth-a2dp";
            case AudioDeviceInfo.TYPE_HDMI: return "hdmi";
            case AudioDeviceInfo.TYPE_USB_DEVICE: return "usb-device";
            case AudioDeviceInfo.TYPE_USB_HEADSET: return "usb-headset";
            case AudioDeviceInfo.TYPE_USB_ACCESSORY: return "usb-accessory";
            case AudioDeviceInfo.TYPE_DOCK: return "dock";
            case 26: return "ble-headset";
            case 27: return "ble-speaker";
            case 30: return "ble-broadcast";
            default: return "type-" + type;
        }
    }

    /**
     * Buffers that never reached the speaker, split by reason: late means the audio queue was already
     * over its 40 ms bound (the decoder thread fell behind or packets arrived in a burst); failed means
     * the output refused the write.
     */
    static String drops(long late, long failed) {
        return "lateDropped=" + late + ", failedWrites=" + failed;
    }
}
