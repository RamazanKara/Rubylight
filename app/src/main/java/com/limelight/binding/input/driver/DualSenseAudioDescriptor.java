package com.limelight.binding.input.driver;

/** The USB Audio 1.0 format used by DualSense/Edge: 48 kHz, four S16LE channels. */
final class DualSenseAudioDescriptor {
    final int controlInterface, streamingInterface, alternateSetting, endpoint, interval, featureUnit;

    private DualSenseAudioDescriptor(int control, int stream, int alternate, int endpoint, int interval, int featureUnit) {
        this.controlInterface = control;
        this.streamingInterface = stream;
        this.alternateSetting = alternate;
        this.endpoint = endpoint;
        this.interval = interval;
        this.featureUnit = featureUnit;
    }

    static DualSenseAudioDescriptor find(byte[] descriptors) { return find(descriptors, false); }
    static DualSenseAudioDescriptor findMicrophone(byte[] descriptors) { return find(descriptors, true); }
    private static DualSenseAudioDescriptor find(byte[] descriptors, boolean microphone) {
        if (descriptors == null) return null;
        int control = -1, stream = -1, alternate = 0, terminal = 0;
        int[] featureUnits = new int[256];
        boolean pcm = false, format = false, inControl = false;
        DualSenseAudioDescriptor found = null;
        for (int offset = 0; offset < descriptors.length;) {
            int length = descriptors[offset] & 0xff;
            if (length < 2 || length > descriptors.length - offset) return null;
            int type = descriptors[offset + 1] & 0xff;
            if (type == 4) {
                if (length < 9) return null;
                stream = -1;
                pcm = format = false;
                inControl = false;
                if (descriptors[offset + 5] == 1 && descriptors[offset + 7] == 0) {
                    if (descriptors[offset + 6] == 1) {
                        control = descriptors[offset + 2] & 0xff;
                        inControl = true;
                    }
                    if (descriptors[offset + 6] == 2) {
                        stream = descriptors[offset + 2] & 0xff;
                        alternate = descriptors[offset + 3] & 0xff;
                    }
                }
            }
            else if (inControl && type == 0x24 && length >= 8 && descriptors[offset + 2] == 6 &&
                    descriptors[offset + 5] == 1 && (descriptors[offset + 6] & 3) == 3) {
                featureUnits[descriptors[offset + 4] & 0xff] = descriptors[offset + 3] & 0xff;
            }
            else if (inControl && type == 0x24 && length >= 9 && descriptors[offset + 2] == 3) {
                featureUnits[descriptors[offset + 3] & 255] = descriptors[offset + 7] & 255;
            }
            else if (stream >= 0 && type == 0x24 && length >= 3) {
                if (descriptors[offset + 2] == 1) {
                    pcm = length >= 7 && descriptors[offset + 5] == 1 && descriptors[offset + 6] == 0;
                    terminal = length >= 7 ? descriptors[offset + 3] & 0xff : 0;
                }
                else if (descriptors[offset + 2] == 2) {
                    format = length == 11 && descriptors[offset + 3] == 1 &&
                            descriptors[offset + 4] == (microphone ? 2 : 4) && descriptors[offset + 5] == 2 &&
                            descriptors[offset + 6] == 16 && descriptors[offset + 7] == 1 &&
                            (descriptors[offset + 8] & 0xff) == 0x80 &&
                            (descriptors[offset + 9] & 0xff) == 0xbb && descriptors[offset + 10] == 0;
                }
            }
            else if (stream >= 0 && alternate != 0 && control >= 0 && pcm && format &&
                    featureUnits[terminal] != 0 && type == 5) {
                if (length < 9) return null;
                int endpoint = descriptors[offset + 2] & 0xff;
                int size = (descriptors[offset + 4] & 0xff) | ((descriptors[offset + 5] & 0xff) << 8);
                if ((endpoint & 15) != 0 && (endpoint & 0xf0) == (microphone ? 0x80 : 0) &&
                        (descriptors[offset + 3] & 0x3f) == (microphone ? 5 : 9) &&
                        size >= (microphone ? 196 : 384) && size <= (microphone ? 196 : 1024) && descriptors[offset + 8] == 0) {
                    if (found != null) return null; // Never guess between audio outputs.
                    found = new DualSenseAudioDescriptor(control, stream, alternate,
                            endpoint, descriptors[offset + 6] & 0xff, featureUnits[terminal]);
                }
            }
            offset += length;
        }
        return found;
    }
}
