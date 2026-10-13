package com.limelight;

import android.app.Application;
import android.content.Intent;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameQualityTest {
    @Test public void pyrowaveExplainsUnsupportedQualityWithoutStartingBitrateOrReconfiguring() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "quality-pc"));
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.videoFormat = PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.MOTION;
        config.width = 1920; config.height = 1080; config.fps = 60;
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        MediaCodecDecoderRenderer renderer = new MediaCodecDecoderRenderer(game, config, null, 0,
                false, false, "", null);
        ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_PYROWAVE);
        ReflectionHelpers.setField(game, "prefConfig", config);
        ReflectionHelpers.setField(game, "decoderRenderer", renderer);
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 73, false));
        ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
        assertNull(ReflectionHelpers.getField(game, "adaptiveQuality"));
        assertNull(ReflectionHelpers.getField(game, "adaptiveBitrate"));
        assertNull(ReflectionHelpers.getField(game, "pyroWaveBitrate"));
        assertEquals(game.getString(R.string.quality_unavailable), ReflectionHelpers.getField(game, "qualityStatus"));
    }

    @Test public void offAndPcOverrideDoNotRequireAConnectedHost() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setIntent(new Intent().putExtra(Game.EXTRA_PC_UUID, "quality-override"));
        PreferenceConfiguration config = new PreferenceConfiguration();
        config.adaptiveQualityMode = AdaptiveQualityController.Mode.DETAIL;
        ReflectionHelpers.setField(game, "prefConfig", config);
        game.getSharedPreferences("AdaptiveQuality", 0).edit().putString("quality-override", "off").commit();
        ReflectionHelpers.callInstanceMethod(game, "startAdaptiveQuality");
        assertNull(ReflectionHelpers.getField(game, "adaptiveQuality"));
        assertNull(ReflectionHelpers.getField(game, "qualityStatus"));
        game.getSharedPreferences("AdaptiveQuality", 0).edit().remove("quality-override").commit();
        assertEquals(AdaptiveQualityController.Mode.DETAIL, ReflectionHelpers.callInstanceMethod(game, "qualityMode"));
    }
}
