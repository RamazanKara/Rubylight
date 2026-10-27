package com.limelight;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.drawable.Drawable;
import android.view.View;
import android.widget.TextView;

import androidx.core.content.ContextCompat;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameConnectionNoticeTest {
    private static Game streamingGame() {
        Game game = Robolectric.buildActivity(Game.class).get();
        game.setTheme(R.style.StreamTheme);
        game.setContentView(R.layout.activity_game);
        ReflectionHelpers.setField(game, "prefConfig", new PreferenceConfiguration());
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "notificationOverlayView", game.findViewById(R.id.notificationOverlay));
        return game;
    }

    @Test
    public void aPoorConnectionShowsTheWordWithTheWarnTriangleOnTheStreamScrim() {
        Game game = streamingGame();
        TextView notice = game.findViewById(R.id.notificationOverlay);
        assertEquals(View.GONE, notice.getVisibility());

        game.connectionStatusUpdate(MoonBridge.CONN_STATUS_POOR);
        assertEquals(View.VISIBLE, notice.getVisibility());
        assertEquals(game.getString(R.string.poor_connection_msg), notice.getText().toString());
        assertEquals(0xffffffff, notice.getCurrentTextColor());

        // The state is a word plus an icon, never colour alone: the triangle leads and draws in warn.
        Drawable icon = notice.getCompoundDrawablesRelative()[0];
        assertNotNull("warn triangle", icon);
        Bitmap bitmap = Bitmap.createBitmap(48, 48, Bitmap.Config.ARGB_8888);
        icon.setBounds(0, 0, 48, 48);
        icon.draw(new Canvas(bitmap));
        int warn = ContextCompat.getColor(game, R.color.night_status_warn);
        boolean drawsWarn = false;
        for (int y = 0; y < 48 && !drawsWarn; y++) for (int x = 0; x < 48; x++) {
            int pixel = bitmap.getPixel(x, y);
            if (Color.alpha(pixel) == 255 && (pixel & 0xffffff) == (warn & 0xffffff)) { drawsWarn = true; break; }
        }
        assertTrue("triangle in status_warn", drawsWarn);
        assertNotNull("sits on the stream scrim", notice.getBackground());

        game.connectionStatusUpdate(MoonBridge.CONN_STATUS_OKAY);
        assertEquals(View.GONE, notice.getVisibility());
    }
}
