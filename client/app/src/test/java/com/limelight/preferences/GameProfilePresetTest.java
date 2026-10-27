package com.limelight.preferences;

import org.junit.Test;

import static org.junit.Assert.*;

public class GameProfilePresetTest {
    private static final int BASE = 20000;
    private static final int QUALITY = GameProfilePreset.qualityBitrate(BASE);

    private static GameProfilePreset afterSaving(GameProfilePreset preset, HostStreamProfile own) {
        preset.apply(own, QUALITY);
        return GameProfilePreset.of(HostStreamProfile.deserialize(own.serialize()), QUALITY);
    }

    @Test
    public void everyPresetIsRecognisedAfterItIsSavedAndRead() {
        for (GameProfilePreset preset : new GameProfilePreset[] {
                GameProfilePreset.COMPETITIVE, GameProfilePreset.SMOOTHNESS, GameProfilePreset.QUALITY}) {
            assertEquals(preset, afterSaving(preset, new HostStreamProfile()));
        }
    }

    @Test
    public void anEmptyOrOtherwiseSetGameIsCustom() {
        assertEquals(GameProfilePreset.CUSTOM, GameProfilePreset.of(new HostStreamProfile(), QUALITY));
        HostStreamProfile other = new HostStreamProfile();
        other.pacing = "balanced";
        assertEquals(GameProfilePreset.CUSTOM, GameProfilePreset.of(other, QUALITY));
        other.bitrate = 12345;
        assertEquals(GameProfilePreset.CUSTOM, GameProfilePreset.of(other, QUALITY));
    }

    @Test
    public void customChangesNothing() {
        HostStreamProfile own = new HostStreamProfile();
        own.bitrate = 9000;
        GameProfilePreset.CUSTOM.apply(own, QUALITY);
        assertEquals(Integer.valueOf(9000), own.bitrate);
        assertNull(own.pacing);
    }

    @Test
    public void competitiveSetsOnlyLatencyPacingAndKeepsTheOtherValues() {
        HostStreamProfile own = new HostStreamProfile();
        own.bitrate = 40000;
        own.vrr = Boolean.TRUE;
        GameProfilePreset.COMPETITIVE.apply(own, QUALITY);
        assertEquals("latency", own.pacing);
        assertEquals("The game's own bitrate stays", Integer.valueOf(40000), own.bitrate);
        assertEquals("Its own adaptive refresh stays", Boolean.TRUE, own.vrr);
        assertEquals(GameProfilePreset.COMPETITIVE, GameProfilePreset.of(own, QUALITY));
    }

    @Test
    public void smoothnessSetsPacingAndAdaptiveRefreshOffAndKeepsTheBitrate() {
        HostStreamProfile own = new HostStreamProfile();
        own.bitrate = 40000;
        GameProfilePreset.SMOOTHNESS.apply(own, QUALITY);
        assertEquals("smoothness", own.pacing);
        assertEquals("Adaptive refresh replaces frame pacing", Boolean.FALSE, own.vrr);
        assertEquals("The game's own bitrate stays", Integer.valueOf(40000), own.bitrate);
        assertEquals(GameProfilePreset.SMOOTHNESS, GameProfilePreset.of(own, QUALITY));
    }

    @Test
    public void qualitySetsOnlyTheBitrateAndKeepsPacingAndAdaptiveRefresh() {
        HostStreamProfile own = new HostStreamProfile();
        own.pacing = "balanced";
        own.vrr = Boolean.TRUE;
        GameProfilePreset.QUALITY.apply(own, QUALITY);
        assertEquals(Integer.valueOf(30000), own.bitrate);
        assertEquals("Frame pacing is not touched", "balanced", own.pacing);
        assertEquals("Neither is adaptive refresh", Boolean.TRUE, own.vrr);
    }

    @Test
    public void presetsWithOverlappingValuesShowCustomInsteadOfOne() {
        HostStreamProfile own = new HostStreamProfile();
        GameProfilePreset.SMOOTHNESS.apply(own, QUALITY);
        GameProfilePreset.QUALITY.apply(own, QUALITY);
        assertEquals(GameProfilePreset.CUSTOM, GameProfilePreset.of(own, QUALITY));
    }

    @Test
    public void qualityIsAHalfMoreAndNeverBeyondTheMaximumTheHostAndTheSliderTake() {
        assertEquals(30000, GameProfilePreset.qualityBitrate(20000));
        assertEquals(HostStreamProfile.MAX_BITRATE_KBPS,
                GameProfilePreset.qualityBitrate(HostStreamProfile.MAX_BITRATE_KBPS));
        assertEquals(HostStreamProfile.MAX_BITRATE_KBPS,
                GameProfilePreset.qualityBitrate(1_900_000));
        assertEquals("Never below the slider's minimum", 500, GameProfilePreset.qualityBitrate(100));
    }

    @Test
    public void theQualityBaseIsThePcsBitrateOrTheMainSettingsAndNeverANetworkProfilesValue() {
        assertEquals(20000, GameProfilePreset.baseBitrate(20000, null));
        HostStreamProfile pc = new HostStreamProfile();
        assertEquals("A PC without a bitrate follows the main settings", 20000, GameProfilePreset.baseBitrate(20000, pc));
        pc.bitrate = 50000;
        assertEquals(50000, GameProfilePreset.baseBitrate(20000, pc));
    }

    @Test
    public void switchingPresetKeepsTheValuesTheNewOneDoesNotName() {
        HostStreamProfile own = new HostStreamProfile();
        own.refreshRateX100 = 12000;
        GameProfilePreset.QUALITY.apply(own, QUALITY);
        GameProfilePreset.COMPETITIVE.apply(own, QUALITY);
        assertEquals("Quality's bitrate stays: Competitive does not name it", Integer.valueOf(QUALITY), own.bitrate);
        assertEquals("Values no preset names stay", Integer.valueOf(12000), own.refreshRateX100);
    }
}
