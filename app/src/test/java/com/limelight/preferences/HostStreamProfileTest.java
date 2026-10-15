package com.limelight.preferences;

import org.junit.Test;
import com.limelight.binding.video.UpscalingPolicy;

import static org.junit.Assert.*;

public class HostStreamProfileTest {
    private static HostStreamProfile legacy(String value) {
        return HostStreamProfile.deserialize(value);
    }

    @Test
    public void oldProfilesStillReadWithEveryValueTheyStored() {
        HostStreamProfile profile = legacy("1920,1080,5994,45500,100,AUTO,false,false,false,false");
        assertNull(profile.vrr);
        assertNull(profile.upscalingMode);
        assertEquals(Boolean.FALSE, profile.nativeResolution);
        assertEquals(Integer.valueOf(5994), profile.refreshRateX100);
        assertEquals(Boolean.TRUE, legacy("1920,1080,5994,45500,100,AUTO,false,false,false,false,true").vrr);
        HostStreamProfile upscaled = legacy("1920,1080,5994,45500,100,AUTO,false,false,false,false,,FSR1,70");
        assertNull(upscaled.vrr);
        assertEquals(UpscalingPolicy.Mode.FSR1, upscaled.upscalingMode);
        assertEquals(Integer.valueOf(70), upscaled.upscalingSharpness);
    }

    @Test
    public void invalidUpscalingProfilesAreRejected() {
        String prefix = "1280,720,6000,10000,100,AUTO,false,false,false,false,";
        for (String suffix : new String[] {"true,FSR1,-1", "true,FSR1,101", "true,FSR1", "true,unknown,50"}) {
            assertThrows(IllegalArgumentException.class, () -> HostStreamProfile.deserialize(prefix + suffix));
        }
        for (String value : new String[] {"v2;upscaling=FSR1:", "v2;upscaling=FSR1:101", "v2;sharpness=101", "v2;hdr=yes",
                "v2;bitrate=499", "v2;res=1x1", "v2;res=native2", "v2;codec=UNKNOWN", "v2;nokey"}) {
            assertThrows(value, IllegalArgumentException.class, () -> HostStreamProfile.deserialize(value));
        }
    }

    @Test
    public void sharpnessAndUpscalingModeOverrideIndependently() {
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.upscalingMode = UpscalingPolicy.Mode.FSR1;
        config.upscalingSharpness = 50;
        HostStreamProfile sharpness = HostStreamProfile.deserialize("v2;sharpness=70");
        assertFalse(sharpness.isEmpty());
        sharpness.copy().applyTo(config);
        assertEquals(UpscalingPolicy.Mode.FSR1, config.upscalingMode);
        assertEquals(70, config.upscalingSharpness);
        config.upscalingMode = UpscalingPolicy.Mode.SGSR1;
        sharpness.applyTo(config);
        assertEquals(UpscalingPolicy.Mode.SGSR1, config.upscalingMode);
        HostStreamProfile.deserialize("v2;upscaling=FSR1").applyTo(config);
        assertEquals(UpscalingPolicy.Mode.FSR1, config.upscalingMode);
        assertEquals(70, config.upscalingSharpness);
        HostStreamProfile.deserialize("v2;upscaling=SGSR1:30").applyTo(config);
        assertEquals(UpscalingPolicy.Mode.SGSR1, config.upscalingMode);
        assertEquals(30, config.upscalingSharpness);
    }

    @Test
    public void unsetValuesFollowTheGlobalSettings() {
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.width = 2184;
        config.height = 1968;
        config.resolutionFollowsScreen = true;
        config.fps = 120;
        config.bitrate = 150000;
        config.vrr = true;
        config.enableHdr = true;
        config.videoFormat = PreferenceConfiguration.FormatOption.FORCE_AV1;
        HostStreamProfile profile = new HostStreamProfile();
        assertTrue(profile.isEmpty());
        assertEquals("v2", profile.serialize());
        profile.bitrate = 80000;
        HostStreamProfile.deserialize(profile.serialize()).applyTo(config);
        assertEquals(80000, config.bitrate);
        assertEquals(2184, config.width);
        assertTrue(config.resolutionFollowsScreen);
        assertTrue(config.vrr);
        assertTrue(config.enableHdr);
        assertEquals(PreferenceConfiguration.FormatOption.FORCE_AV1, config.videoFormat);
    }

    @Test
    public void ownValuesSurviveStorageAndOverrideTheGlobalOnes() {
        HostStreamProfile profile = new HostStreamProfile();
        profile.nativeResolution = false;
        profile.width = 1600;
        profile.height = 2560;
        profile.refreshRateX100 = 11988;
        profile.bitrate = 280000;
        profile.renderScale = 150;
        profile.codec = PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
        profile.virtualDisplay = profile.hdr = profile.fullRange = profile.yuv444 = true;
        profile.vrr = false;
        profile.upscalingMode = UpscalingPolicy.Mode.SGSR1;
        profile.upscalingSharpness = 30;
        HostStreamProfile restored = HostStreamProfile.deserialize(profile.serialize());
        assertEquals(profile.serialize(), restored.serialize());
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.useTextureView = true;
        config.vrr = true;
        config.resolutionFollowsScreen = true;
        restored.applyTo(config);
        assertEquals(1600, config.width);
        assertEquals(2560, config.height);
        assertFalse(config.resolutionFollowsScreen);
        assertEquals(11988, config.launchRefreshRateX100);
        assertEquals(120, config.fps);
        assertEquals(280000, config.bitrate);
        assertEquals(150, config.virtualDisplayScale);
        assertEquals(PreferenceConfiguration.FormatOption.FORCE_PYROWAVE, config.videoFormat);
        assertTrue(config.virtualDisplay);
        assertTrue(config.enableHdr);
        assertTrue(config.fullRange);
        assertTrue(config.enableYuv444);
        assertFalse(config.vrr);
        assertEquals(UpscalingPolicy.Mode.SGSR1, config.upscalingMode);
        assertEquals(30, config.upscalingSharpness);
        assertFalse(config.useTextureView);
    }

    @Test
    public void ownSwitchesCanTurnGlobalOnesOff() {
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.useTextureView = true;
        config.virtualDisplay = config.enableHdr = config.fullRange = config.enableYuv444 = config.vrr = true;
        HostStreamProfile.deserialize("v2;virtual=false;hdr=false;full_range=false;yuv444=false;vrr=false").applyTo(config);
        assertTrue(config.useTextureView);
        assertFalse(config.virtualDisplay);
        assertFalse(config.enableHdr);
        assertFalse(config.fullRange);
        assertFalse(config.enableYuv444);
        assertFalse(config.vrr);
    }

    @Test
    public void oldProfilesKeepOnlyTheValuesThatDifferFromTheGlobalOnes() {
        PreferenceConfiguration global = new PreferenceConfiguration();
        global.resolutionFollowsScreen = true;
        global.width = 2184;
        global.height = 1968;
        global.fps = 120;
        global.bitrate = 150000;
        global.virtualDisplayScale = 100;
        global.videoFormat = PreferenceConfiguration.FormatOption.AUTO;
        global.vrr = false;
        // A native profile saved with every other value copied: nothing of its own is left.
        assertEquals("", HostStreamProfile.migrateToOverrides(
                "2184,1968,12000,150000,100,AUTO,false,false,false,false,false,OFF,50,native", global));
        // Before the native marker existed, profiles stored the resolved screen size.
        assertEquals("", HostStreamProfile.migrateToOverrides(
                "2184,1968,12000,150000,100,AUTO,false,false,false,false,false,OFF,50", global));
        // PyroWave and its bitrate were chosen for the PC; the stale VRR copy follows the global switch.
        assertEquals("v2;bitrate=800000;codec=FORCE_PYROWAVE", HostStreamProfile.migrateToOverrides(
                "2184,1968,12000,800000,100,FORCE_PYROWAVE,false,false,false,false,true,OFF,50,native", global));
        // A fixed size stays fixed.
        assertEquals("v2;res=2560x1440", HostStreamProfile.migrateToOverrides(
                "2560,1440,12000,150000,100,AUTO,false,false,false,false", global));
        // Converted profiles are left alone.
        assertNull(HostStreamProfile.migrateToOverrides("v2;bitrate=800000", global));
    }

    @Test
    public void newerEntriesAreIgnoredAndEmptyProfilesSerializeAsTheFormatOnly() {
        HostStreamProfile profile = HostStreamProfile.deserialize("v2;hdr=true;future=1");
        assertEquals(Boolean.TRUE, profile.hdr);
        assertEquals("v2;hdr=true", profile.serialize());
        assertTrue(HostStreamProfile.deserialize("v2").isEmpty());
    }

    @Test
    public void fractionalRefreshIsExactAndAcceptsLocalizedDecimalSeparator() {
        assertEquals(5994, HostStreamProfile.parseRefreshRate("59.94"));
        assertEquals(11988, HostStreamProfile.parseRefreshRate("119,88"));
        assertEquals(24000, HostStreamProfile.parseRefreshRate(" 240 "));
        assertEquals("59.94", HostStreamProfile.formatRefreshRate(5994));
        assertEquals("120", HostStreamProfile.formatRefreshRate(12000));
    }

    @Test
    public void bitrateIsEnteredInMbpsAndStoredInKbps() {
        assertEquals(280000, HostStreamProfile.parseBitrateMbps("280"));
        assertEquals(10500, HostStreamProfile.parseBitrateMbps("10,5"));
        assertEquals(500, HostStreamProfile.parseBitrateMbps(" 0.5 "));
        assertEquals(1600000, HostStreamProfile.parseBitrateMbps("1600"));
        assertEquals("10", HostStreamProfile.formatBitrateMbps(10000));
        assertEquals("12.345", HostStreamProfile.formatBitrateMbps(12345));
        for (String value : new String[] {"0.4", "2000.001", "1.0005", "NaN", ""}) {
            assertThrows(value, IllegalArgumentException.class, () -> HostStreamProfile.parseBitrateMbps(value));
        }
    }

    @Test
    public void invalidRefreshCannotBeSilentlyRoundedOrOverflow() {
        for (String value : new String[] {"59.999", "0", "1000.01", "NaN", "Infinity", "2147483648", ""}) {
            assertThrows(value, IllegalArgumentException.class, () -> HostStreamProfile.parseRefreshRate(value));
        }
    }

    @Test
    public void malformedAndOutOfRangeStoredValuesAreRejected() {
        for (String value : new String[] {
                "1280,720,6000", "1280,720,6000,10000,100,UNKNOWN,false,false,false,false",
                "0,720,6000,10000,100,AUTO,false,false,false,false",
                "1280,16385,6000,10000,100,AUTO,false,false,false,false",
                "1280,720,6000,499,100,AUTO,false,false,false,false",
                "1280,720,6000,2000001,100,AUTO,false,false,false,false",
                "1280,720,6000,10000,201,AUTO,false,false,false,false"}) {
            assertThrows(IllegalArgumentException.class, () -> HostStreamProfile.deserialize(value));
        }
    }

    @Test
    public void nativeProfilesRoundTrip() {
        HostStreamProfile read = HostStreamProfile.deserialize("v2;res=native;codec=FORCE_PYROWAVE");
        assertEquals(Boolean.TRUE, read.nativeResolution);
        assertEquals("v2;res=native;codec=FORCE_PYROWAVE", read.serialize());
        PreferenceConfiguration config = new PreferenceConfiguration();
        read.applyTo(config);
        assertTrue(config.resolutionFollowsScreen);
        assertEquals(Boolean.TRUE, HostStreamProfile.deserialize(
                "2184,1968,12000,80000,100,AUTO,false,false,false,false,native").nativeResolution);
    }

    @Test
    public void profilesStuckAt1080pBecomeNativeOnce() {
        String stuck = "1920,1080,12000,150000,100,FORCE_PYROWAVE,true,false,false,true,true,OFF,50";
        String migrated = HostStreamProfile.migrateToNative(stuck);
        assertEquals(stuck + ",native", migrated);
        assertNull(HostStreamProfile.migrateToNative(migrated));
        assertNull(HostStreamProfile.migrateToNative("2560,1440,12000,150000,100,AUTO,false,false,false,false"));
    }
    @Test
    public void framePacingIsAValueOfItsOwnThatOlderBuildsSkip() {
        HostStreamProfile profile = HostStreamProfile.deserialize("v2;bitrate=20000;pacing=cap-fps");
        assertEquals("cap-fps", profile.pacing);
        assertEquals("v2;bitrate=20000;pacing=cap-fps", profile.serialize());
        assertEquals(2, profile.count());
        PreferenceConfiguration config = new PreferenceConfiguration();
        profile.applyTo(config);
        assertEquals(PreferenceConfiguration.FRAME_PACING_CAP_FPS, config.framePacing);
        assertThrows(IllegalArgumentException.class, () -> HostStreamProfile.deserialize("v2;pacing=fast"));
        // A value from a newer build is left to the layers below.
        assertEquals("v2;hdr=true", HostStreamProfile.deserialize("v2;hdr=true;future=1").serialize());
    }

    @Test
    public void everyFieldCanBeCopiedAloneAndFallsThroughWhenUnset() {
        HostStreamProfile full = HostStreamProfile.deserialize("v2;res=1920x1080;refresh=6000;bitrate=20000;" +
                "scale=150;codec=FORCE_AV1;virtual=true;hdr=true;full_range=true;yuv444=true;vrr=false;" +
                "upscaling=FSR1;sharpness=60;pacing=balanced");
        assertEquals(HostStreamProfile.Field.values().length, full.count());
        HostStreamProfile copy = new HostStreamProfile();
        for (HostStreamProfile.Field field : HostStreamProfile.Field.values()) {
            assertTrue(field.name(), full.has(field));
            assertFalse(field.name(), copy.has(field));
            copy.take(field, full);
            assertTrue(field.name(), copy.has(field));
        }
        assertEquals(full.serialize(), copy.serialize());
        for (HostStreamProfile.Field field : HostStreamProfile.Field.values()) {
            copy.take(field, new HostStreamProfile());
        }
        assertTrue(copy.isEmpty());
    }
}
