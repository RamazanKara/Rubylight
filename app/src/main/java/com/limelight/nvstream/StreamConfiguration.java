package com.limelight.nvstream;

import com.limelight.nvstream.http.FecChoice;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.HostDisplayChoice;

public class StreamConfiguration {
    public static final int INVALID_APP_ID = 0;

    public static final int STREAM_CFG_LOCAL = 0;
    public static final int STREAM_CFG_REMOTE = 1;
    public static final int STREAM_CFG_AUTO = 2;

    // Highest bitrate the client asks for, at setup or mid-stream. Each host clamps to its own
    // ceiling and the client follows the bitrate it reports back.
    public static final int MAX_BITRATE_KBPS = 2000000;
    
    private NvApp app;
    private int width, height;
    private int refreshRate;
    private int launchRefreshRate;
    private int launchRefreshRateX100;
    private int clientRefreshRateX100;
    private boolean virtualDisplay;
    private int virtualDisplayScale = 100;
    private boolean vrr;
    // The PC's display choice as saved for the PC the stream was started from; null: look it up.
    private HostDisplayChoice hostDisplay;
    // The video FEC to ask a PC that lets the client choose; null sends nothing.
    private FecChoice fecChoice;
    private boolean rateChange;
    private int bitrate;
    // The bitrate is the automatic one: a new stream may time the link and start lower.
    private boolean probeFirstBitrate;
    private boolean sops;
    private boolean enableAdaptiveResolution;
    private boolean playLocalAudio;
    private int maxPacketSize;
    private int remote;
    private MoonBridge.AudioConfiguration audioConfiguration;
    private int supportedVideoFormats;
    // The 10-bit formats offered are for 10-bit SDR, not HDR: the launch does not ask for HDR.
    private boolean sdr10Bit;
    // Packed 4:4:4 as this session uses it (off for a session reconnected after the GPU path failed).
    private boolean packed444;
    private int attachedGamepadMask;
    private int encryptionFlags;
    private int colorRange;
    private int colorSpace;
    private boolean persistGamepadsAfterDisconnect;
    private boolean resumeOnly;
    private boolean allowIdleSessionEnd = true;

    public static class Builder {
        private StreamConfiguration config = new StreamConfiguration();
        
        public StreamConfiguration.Builder setApp(NvApp app) {
            config.app = app;
            return this;
        }

        /** Recovery must never launch, replace or terminate an application. */
        public StreamConfiguration.Builder setResumeOnly(boolean resumeOnly) {
            config.resumeOnly = resumeOnly;
            return this;
        }

        /** A retry after an explicit media failure must still report that the session was lost. */
        public StreamConfiguration.Builder setAllowIdleSessionEnd(boolean allow) {
            config.allowIdleSessionEnd = allow;
            return this;
        }
        
        public StreamConfiguration.Builder setRemoteConfiguration(int remote) {
            config.remote = remote;
            return this;
        }
        
        public StreamConfiguration.Builder setResolution(int width, int height) {
            config.width = width;
            config.height = height;
            return this;
        }
        
        public StreamConfiguration.Builder setRefreshRate(int refreshRate) {
            config.refreshRate = refreshRate;
            return this;
        }

        public StreamConfiguration.Builder setLaunchRefreshRate(int refreshRate) {
            config.launchRefreshRate = refreshRate;
            return this;
        }

        public StreamConfiguration.Builder setLaunchRefreshRateX100(int refreshRateX100) {
            config.launchRefreshRateX100 = refreshRateX100;
            return this;
        }
        
        public StreamConfiguration.Builder setBitrate(int bitrate) {
            config.bitrate = bitrate;
            return this;
        }
        
        /** The bitrate above is automatic, so a new stream times the link first and starts no higher than it holds. */
        public StreamConfiguration.Builder setProbeFirstBitrate(boolean probe) {
            config.probeFirstBitrate = probe;
            return this;
        }

        public StreamConfiguration.Builder setFecChoice(FecChoice choice) {
            config.fecChoice = choice;
            return this;
        }

        /** Asks for mid-stream rate changes (0x553C), which only the Rust engine sends. */
        public StreamConfiguration.Builder setRateChange(boolean rateChange) {
            config.rateChange = rateChange;
            return this;
        }

        public StreamConfiguration.Builder setEnableSops(boolean enable) {
            config.sops = enable;
            return this;
        }
        
        public StreamConfiguration.Builder enableAdaptiveResolution(boolean enable) {
            config.enableAdaptiveResolution = enable;
            return this;
        }
        
        public StreamConfiguration.Builder enableLocalAudioPlayback(boolean enable) {
            config.playLocalAudio = enable;
            return this;
        }
        
        public StreamConfiguration.Builder setMaxPacketSize(int maxPacketSize) {
            config.maxPacketSize = maxPacketSize;
            return this;
        }

        public StreamConfiguration.Builder setAttachedGamepadMask(int attachedGamepadMask) {
            config.attachedGamepadMask = attachedGamepadMask;
            return this;
        }

        public StreamConfiguration.Builder setAttachedGamepadMaskByCount(int gamepadCount) {
            config.attachedGamepadMask = 0;
            for (int i = 0; i < 4; i++) {
                if (gamepadCount > i) {
                    config.attachedGamepadMask |= 1 << i;
                }
            }
            return this;
        }

        public StreamConfiguration.Builder setPersistGamepadsAfterDisconnect(boolean value) {
            config.persistGamepadsAfterDisconnect = value;
            return this;
        }

        public StreamConfiguration.Builder setClientRefreshRateX100(int refreshRateX100) {
            config.clientRefreshRateX100 = refreshRateX100;
            return this;
        }

        public StreamConfiguration.Builder setVirtualDisplay(boolean enabled, int scale) {
            config.virtualDisplay = enabled;
            config.virtualDisplayScale = scale;
            return this;
        }

        public StreamConfiguration.Builder setVrr(boolean enabled) {
            config.vrr = enabled;
            return this;
        }

        public StreamConfiguration.Builder setHostDisplay(HostDisplayChoice choice) {
            config.hostDisplay = choice;
            return this;
        }

        public StreamConfiguration.Builder setAudioConfiguration(MoonBridge.AudioConfiguration audioConfig) {
            config.audioConfiguration = audioConfig;
            return this;
        }
        
        public StreamConfiguration.Builder setSupportedVideoFormats(int supportedVideoFormats) {
            config.supportedVideoFormats = supportedVideoFormats;
            return this;
        }

        public StreamConfiguration.Builder setSdr10Bit(boolean sdr10Bit) {
            config.sdr10Bit = sdr10Bit;
            return this;
        }

        /** Whether this stream asks for experimental packed 4:4:4: the session's choice, not the saved setting. */
        public StreamConfiguration.Builder setPacked444(boolean packed444) {
            config.packed444 = packed444;
            return this;
        }

        public StreamConfiguration.Builder setColorRange(int colorRange) {
            config.colorRange = colorRange;
            return this;
        }

        public StreamConfiguration.Builder setColorSpace(int colorSpace) {
            config.colorSpace = colorSpace;
            return this;
        }

        public StreamConfiguration build() {
            return config;
        }
    }
    
    private StreamConfiguration() {
        // Set default attributes
        this.app = new NvApp("Steam");
        this.width = 1280;
        this.height = 720;
        this.refreshRate = 60;
        this.launchRefreshRate = 60;
        this.bitrate = 10000;
        this.maxPacketSize = 1024;
        this.remote = STREAM_CFG_AUTO;
        this.sops = true;
        this.enableAdaptiveResolution = false;
        this.audioConfiguration = new MoonBridge.AudioConfiguration(2, 0x3);
        this.supportedVideoFormats = MoonBridge.VIDEO_FORMAT_H264;
        this.attachedGamepadMask = 0;
    }
    
    public int getWidth() {
        return width;
    }
    
    public int getHeight() {
        return height;
    }
    
    public int getRefreshRate() {
        return refreshRate;
    }

    public int getLaunchRefreshRate() {
        return launchRefreshRate;
    }

    public int getLaunchRefreshRateX100() {
        return launchRefreshRateX100;
    }
    
    public int getBitrate() {
        return bitrate;
    }
    
    public boolean probesFirstBitrate() {
        return probeFirstBitrate;
    }

    public FecChoice getFecChoice() {
        return fecChoice;
    }

    public boolean getRateChange() {
        return rateChange;
    }

    public int getMaxPacketSize() {
        return maxPacketSize;
    }

    public NvApp getApp() {
        return app;
    }

    public boolean isResumeOnly() {
        return resumeOnly;
    }

    public boolean isIdleSessionEndAllowed() {
        return allowIdleSessionEnd;
    }
    
    public boolean getSops() {
        return sops;
    }
    
    public boolean getAdaptiveResolutionEnabled() {
        return enableAdaptiveResolution;
    }
    
    public boolean getPlayLocalAudio() {
        return playLocalAudio;
    }
    
    public int getRemote() {
        return remote;
    }

    public MoonBridge.AudioConfiguration getAudioConfiguration() {
        return audioConfiguration;
    }
    
    public int getSupportedVideoFormats() {
        return supportedVideoFormats;
    }

    public boolean isSdr10Bit() {
        return sdr10Bit;
    }

    public boolean isPacked444() {
        return packed444;
    }

    public int getAttachedGamepadMask() {
        return attachedGamepadMask;
    }

    public boolean getPersistGamepadsAfterDisconnect() {
        return persistGamepadsAfterDisconnect;
    }

    public int getClientRefreshRateX100() {
        return clientRefreshRateX100;
    }

    public boolean getVirtualDisplay() {
        return virtualDisplay;
    }

    public int getVirtualDisplayScale() {
        return virtualDisplayScale;
    }

    public boolean getVrr() {
        return vrr;
    }

    public HostDisplayChoice getHostDisplay() {
        return hostDisplay;
    }

    public int getColorRange() {
        return colorRange;
    }

    public int getColorSpace() {
        return colorSpace;
    }
}
