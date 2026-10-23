package com.limelight.preferences;

import org.junit.Test;

import static org.junit.Assert.*;

public class GameProfilePresetTest {
    private static final int BASE = 20000;

    private static GameProfilePreset afterSaving(GameProfilePreset preset, HostStreamProfile own) {
        preset.apply(own, BASE);
        return GameProfilePreset.of(HostStreamProfile.deserialize(own.serialize()), BASE);
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
        assertEquals(GameProfilePreset.CUSTOM, GameProfilePreset.of(new HostStreamProfile(), BASE));
        HostStreamProfile other = new HostStreamProfile();
        other.pacing = "balanced";
        assertEquals(GameProfilePreset.CUSTOM, GameProfilePreset.of(other, BASE));
        other.bitrate = 12345;
        assertEquals(GameProfilePreset.CUSTOM, GameProfilePreset.of(other, BASE));
    }

    @Test
    public void customChangesNothing() {
        HostStreamProfile own = new HostStreamProfile();
        own.bitrate = 9000;
        GameProfilePreset.CUSTOM.apply(own, BASE);
        assertEquals(Integer.valueOf(9000), own.bitrate);
        assertNull(own.pacing);
    }

    @Test
    public void competitiveKeepsLatencyPacingEvenIfTheMainSettingChanges() {
        HostStreamProfile own = new HostStreamProfile();
        GameProfilePreset.COMPETITIVE.apply(own, BASE);
        assertEquals("latency", own.pacing);
        assertTrue(own.has(HostStreamProfile.Field.PACING));
        assertNull(own.bitrate);
        assertNull(own.vrr);
    }

    @Test
    public void smoothnessTurnsAdaptiveRefreshOffBecauseItReplacesFramePacing() {
        HostStreamProfile own = new HostStreamProfile();
        GameProfilePreset.SMOOTHNESS.apply(own, BASE);
        assertEquals("smoothness", own.pacing);
        assertEquals(Boolean.FALSE, own.vrr);
        assertNull(own.bitrate);
    }

    @Test
    public void qualityRaisesOnlyTheBitrateByHalfUpToTheHostLimit() {
        HostStreamProfile own = new HostStreamProfile();
        GameProfilePreset.QUALITY.apply(own, BASE);
        assertEquals(Integer.valueOf(30000), own.bitrate);
        assertNull("Frame pacing is not touched", own.pacing);
        assertNull(own.vrr);
        GameProfilePreset.QUALITY.apply(own, HostStreamProfile.MAX_BITRATE_KBPS);
        assertEquals(Integer.valueOf(HostStreamProfile.MAX_BITRATE_KBPS), own.bitrate);
        assertEquals(GameProfilePreset.QUALITY, GameProfilePreset.of(own, HostStreamProfile.MAX_BITRATE_KBPS));
    }

    @Test
    public void switchingPresetReplacesWhatTheLastOneSet() {
        HostStreamProfile own = new HostStreamProfile();
        own.refreshRateX100 = 12000;
        GameProfilePreset.QUALITY.apply(own, BASE);
        GameProfilePreset.SMOOTHNESS.apply(own, BASE);
        assertNull("Quality's bitrate is gone", own.bitrate);
        GameProfilePreset.COMPETITIVE.apply(own, BASE);
        assertNull(own.vrr);
        assertEquals("Values a preset does not own stay", Integer.valueOf(12000), own.refreshRateX100);
        assertEquals(GameProfilePreset.COMPETITIVE, GameProfilePreset.of(own, BASE));
    }
}
