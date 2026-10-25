package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.preference.PreferenceManager;
import android.provider.Settings;
import android.view.KeyEvent;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.TextView;

import com.limelight.AppView;
import com.limelight.PcViewLifecycleTest;
import com.limelight.grid.AppGridAdapter;
import com.limelight.grid.assets.CachedAppAssetLoader;
import com.limelight.grid.assets.MemoryAssetLoader;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.TimeUnit;

/** The focus line and the backdrop follow the grid's selection without costing a held key anything. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 30, application = Application.class, qualifiers = "w960dp-h540dp-television-xhdpi",
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
public class LibraryAmbienceTest {
    /** Counts the lines it is given, which is what a frame's worth of text layout costs. */
    private static final class CountingText extends TextView {
        int sets;

        CountingText(Context context) {
            super(context);
        }

        @Override
        public void setText(CharSequence text, BufferType type) {
            sets++;
            super.setText(text, type);
        }
    }

    private ActivityController<UiNavigationTest.Screen> controller;
    private AdaptiveGridView grid;
    private BackdropView backdrop;
    private CountingText title;
    private LibraryAmbience ambience;
    private AppGridAdapter adapter;
    /** What the stub loader says of every game: its art is known to be missing, or may still arrive. */
    private boolean noArt = true;

    @Before
    public void openLibrary() {
        Settings.Global.putFloat(RuntimeEnvironment.getApplication().getContentResolver(),
                Settings.Global.ANIMATOR_DURATION_SCALE, 1f);
        org.robolectric.util.ReflectionHelpers.callStaticMethod(android.animation.ValueAnimator.class, "setDurationScale",
                org.robolectric.util.ReflectionHelpers.ClassParameter.from(float.class, 1f));
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
        controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible();
        UiNavigationTest.Screen screen = controller.get();

        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "library-host";
        computer.name = "PC";
        adapter = new AppGridAdapter(screen, new PreferenceConfiguration(), computer, "test", false);
        ReflectionHelpers.setField(adapter, "loader",
                new CachedAppAssetLoader(computer, 1, null, new MemoryAssetLoader(), null, null) {
                    @Override public void queueCacheLoad(NvApp app) { }
                    // No loader tasks: their background threads would throw without a disk loader, and the
                    // app's crash handler turns that into a dead test JVM.
                    @Override public boolean populateImageView(NvApp app, android.widget.ImageView view, TextView text) { return false; }
                    @Override public boolean hasNoArt(NvApp app) { return noArt; }
                });
        for (int i = 0; i < 90; i++) adapter.addApp(new AppView.AppObject(new NvApp("Game " + i, 100 + i, false)));

        grid = new AdaptiveGridView(screen);
        grid.setColumnPolicy(false, false);
        grid.setPadding(0, 32, 0, 64);
        grid.setClipToPadding(false);
        grid.setAdapter(adapter);
        backdrop = new BackdropView(screen);
        backdrop.setWorker(Runnable::run);
        title = new CountingText(screen);
        ambience = new LibraryAmbience(backdrop, title, null);
        ambience.bind(grid, adapter);

        FrameLayout container = new FrameLayout(screen);
        container.addView(backdrop, new FrameLayout.LayoutParams(1728, 700));
        container.addView(grid, new FrameLayout.LayoutParams(1728, 700));
        screen.setContentView(container);
        layout();
        grid.requestFocusFromTouch();
        grid.setSelection(0);
        layout();
        pass(2000);
    }

    @After
    public void closeLibrary() {
        controller.close();
    }

    private void layout() {
        grid.measure(View.MeasureSpec.makeMeasureSpec(1728, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(700, View.MeasureSpec.EXACTLY));
        grid.layout(0, 0, 1728, 700);
    }

    private static void pass(long ms) {
        ShadowLooper.idleMainLooper(ms, TimeUnit.MILLISECONDS);
    }

    @Test
    public void aHeldKeyAcrossTheLibraryChangesTheBackdropOnceAtItsEnd() {
        // Idling the looper at the first press lets the paused test clock jump by a couple of hundred milliseconds,
        // which settles the backdrop by itself; BackdropViewTest holds the clock still and counts the whole burst.
        // Reduce motion has no glide, and the debounce under test is the same.
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
        press(0);
        int before = backdrop.changeCount();
        for (int i = 1; i < 30; i++) {
            press(i);
            assertEquals("no change while the keys keep coming", before, backdrop.changeCount());
        }
        assertTrue("the selection moved a long way", grid.getSelectedItemPosition() > 8);
        pass(BackdropView.FAST_SETTLE_MS + 700);
        assertEquals("one change at the end of the run", before + 1, backdrop.changeCount());
        assertEquals("and the line says the game it ended on", selectedName(), title.getText().toString());
    }

    @Test
    public void aGameWhoseCoverIsStillLoadingLeavesTheBackdropAloneUntilTheCoverIsKnownMissing() {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
        noArt = false;
        int before = backdrop.changeCount();
        press(1);
        pass(BackdropView.FAST_SETTLE_MS + 700);
        assertEquals("loading: the backdrop is not swapped for the canvas", before, backdrop.changeCount());

        // The loader gives up on the game (nothing on the host either) and says so through the art listener.
        noArt = true;
        java.util.function.Consumer<NvApp> listener = ReflectionHelpers.getField(adapter, "artListener");
        listener.accept(((AppView.AppObject) grid.getSelectedItem()).app);
        pass(BackdropView.FAST_SETTLE_MS + 700);
        assertEquals("no art: the canvas", before + 1, backdrop.changeCount());
    }

    private void press(int i) {
        int key = i % 2 == 0 ? KeyEvent.KEYCODE_DPAD_DOWN : KeyEvent.KEYCODE_DPAD_RIGHT;
        long now = android.os.SystemClock.uptimeMillis();
        grid.onKeyDown(key, new KeyEvent(now, now, KeyEvent.ACTION_DOWN, key, i));
        layout();
        ShadowLooper.idleMainLooper();
        // 30 ms between keys; the looper is not idled for it, so the paused clock moves exactly that much.
        org.robolectric.shadows.ShadowSystemClock.advanceBy(java.time.Duration.ofMillis(30));
    }

    private String selectedName() {
        return ((AppView.AppObject) grid.getSelectedItem()).app.getAppName();
    }

    @Test
    public void severalSelectionsWithinAFrameSetTheFocusLineOnce() {
        title.sets = 0;
        android.widget.AdapterView.OnItemSelectedListener listener = grid.getOnItemSelectedListener();
        for (int position = 1; position <= 6; position++) {
            grid.setSelection(position);
            listener.onItemSelected(grid, null, position, position);
        }
        pass(40);
        assertEquals("one line of text for six selections", 1, title.sets);
        assertEquals(((AppView.AppObject) adapter.getItem(6)).app.getAppName(), title.getText().toString());
    }

    @Test
    public void aSearchWithNoMatchEmptiesTheFocusLine() {
        ambience.clear();
        assertEquals("", title.getText().toString());
        ambience.refresh();
        pass(40);
        assertEquals("a refresh with a selection brings it back", selectedName(), title.getText().toString());
    }
}
