package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.drawable.BitmapDrawable;
import android.preference.PreferenceManager;
import android.provider.Settings;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.ImageView;

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

import java.util.concurrent.TimeUnit;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 30, application = Application.class)
public class BackdropViewTest {
    private ActivityController<UiNavigationTest.Screen> controller;
    private BackdropView backdrop;

    @Before
    public void attach() {
        Settings.Global.putFloat(RuntimeEnvironment.getApplication().getContentResolver(),
                Settings.Global.ANIMATOR_DURATION_SCALE, 1f);
        org.robolectric.util.ReflectionHelpers.callStaticMethod(android.animation.ValueAnimator.class, "setDurationScale",
                org.robolectric.util.ReflectionHelpers.ClassParameter.from(float.class, 1f));
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
        BackdropView.clearHandoff();
        controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible();
        backdrop = new BackdropView(controller.get());
        backdrop.setWorker(Runnable::run);
        controller.get().addContentView(backdrop, new FrameLayout.LayoutParams(1920, 1080));
    }

    @After
    public void detach() {
        controller.close();
    }

    private static void pass(long ms) {
        ShadowLooper.idleMainLooper(ms, TimeUnit.MILLISECONDS);
    }

    private static Bitmap cover() {
        Bitmap bitmap = Bitmap.createBitmap(30, 45, Bitmap.Config.ARGB_8888);
        bitmap.eraseColor(0xFF336699);
        return bitmap;
    }

    private ImageView back() {
        return (ImageView) backdrop.getChildAt(0);
    }

    private ImageView front() {
        return (ImageView) backdrop.getChildAt(1);
    }

    @Test
    public void aHeldKeyChangesTheBackdropForItsFirstPressAndOnceMoreAtTheEnd() {
        Bitmap[] covers = {cover(), cover(), cover()};
        // The first press is not a fast one: it settles on its own game, as a single tap would.
        backdrop.focus("game0", covers[0], false);
        // Android starts repeating a held key after about 400 ms.
        pass(400);
        assertEquals(1, backdrop.changeCount());
        for (int key = 1; key < 30; key++) {
            // The repeat comes about every 30 ms and asks for the game the selection is on now.
            backdrop.focus("game" + key, covers[key % covers.length], true);
            pass(30);
            assertEquals("nothing changes while the keys keep coming", 1, backdrop.changeCount());
        }
        pass(BackdropView.FAST_SETTLE_MS + 100);
        pass(BackdropView.FIRST_FADE_MS + 50);
        assertEquals("at most two changes for the whole press: its start and its end", 2, backdrop.changeCount());
    }

    @Test
    public void aCoverStillLoadingKeepsTheBackdropAndOnlyAGameWithoutArtGetsTheCanvas() {
        backdrop.focus("a", cover(), false);
        pass(1000);
        Bitmap shown = ((BitmapDrawable) back().getDrawable()).getBitmap();
        assertEquals(1, backdrop.changeCount());

        backdrop.focus("b", null, false, false);
        pass(1000);
        assertEquals("not loaded yet: nothing changes", 1, backdrop.changeCount());
        assertSame(shown, ((BitmapDrawable) back().getDrawable()).getBitmap());

        backdrop.focus("b", cover(), false, false);
        pass(1000);
        assertEquals("the cover arrived", 2, backdrop.changeCount());

        backdrop.focus("c", null, true, false);
        pass(1000);
        assertEquals("a game with no art gets the canvas", 3, backdrop.changeCount());
    }

    @Test
    public void aSinglePressChangesItAfterTheShortSettle() {
        backdrop.focus("a", cover(), false);
        pass(BackdropView.SETTLE_MS - 10);
        assertEquals(0, backdrop.changeCount());
        pass(20);
        assertEquals(1, backdrop.changeCount());
    }

    @Test
    public void theSameGameAskedForAgainDoesNotChangeIt() {
        Bitmap cover = cover();
        backdrop.focus("a", cover, false);
        pass(1000);
        backdrop.focus("a", cover, false);
        pass(1000);
        assertEquals(1, backdrop.changeCount());
    }

    // Robolectric's paused looper does not tick view animators, so the fade itself is checked on a
    // device (frame sequences in the design shots); here the hand-over of the two layers is.
    @Test
    public void aChangeEndsWithTheNewBackdropAtRestOnTheBackLayer() {
        assertNotNull("attached to a window", backdrop.getWindowToken());
        assertEquals(false, Motion.reduced(controller.get()));
        backdrop.focus("a", cover(), false);
        pass(1000);
        assertEquals("the first one is at rest on the back layer", View.GONE, front().getVisibility());
        assertNotNull(back().getDrawable());
        Bitmap first = ((BitmapDrawable) back().getDrawable()).getBitmap();

        backdrop.focus("b", cover(), false);
        pass(BackdropView.SETTLE_MS + BackdropView.ZOOM_MS + 100);
        assertEquals("the fade is over: the front layer is free", View.GONE, front().getVisibility());
        assertNull(front().getDrawable());
        assertEquals(1f, front().getScaleX(), 0f);
        assertTrue(back().getDrawable() instanceof BitmapDrawable);
        assertTrue("the new one rests in place of the old", ((BitmapDrawable) back().getDrawable()).getBitmap() != first);
        assertEquals(2, backdrop.changeCount());
    }

    @Test
    public void aChangeCutShortStillEndsOnTheLastGame() {
        backdrop.focus("a", cover(), false);
        pass(1000);
        backdrop.focus("b", cover(), false);
        pass(BackdropView.SETTLE_MS + 60);
        backdrop.focus("c", cover(), false);
        pass(BackdropView.SETTLE_MS + 5);
        pass(1000);
        assertEquals(3, backdrop.changeCount());
        assertEquals(View.GONE, front().getVisibility());
        assertNull(front().getDrawable());
        assertNotNull(back().getDrawable());
    }

    @Test
    public void withReducedMotionTheNewBackdropIsThereAtOnce() {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
        backdrop.focus("a", cover(), false);
        pass(BackdropView.SETTLE_MS + 5);
        assertEquals(1, backdrop.changeCount());
        assertNotNull(back().getDrawable());
        assertEquals("no front layer, no fade", View.GONE, front().getVisibility());
        backdrop.focus("b", cover(), false);
        pass(BackdropView.SETTLE_MS + 5);
        assertEquals(2, backdrop.changeCount());
        assertEquals(View.GONE, front().getVisibility());
        assertEquals(1f, front().getScaleX(), 0f);
    }

    @Test
    @Config(qualifiers = "w960dp-h540dp-xhdpi")
    public void theBackdropFillsTheWindowWhateverPaddingItsParentsAdd() {
        FrameLayout content = controller.get().findViewById(android.R.id.content);
        content.setPadding(120, 80, 120, 80);
        View decor = controller.get().getWindow().getDecorView();
        int width = decor.getResources().getDisplayMetrics().widthPixels;
        int height = decor.getResources().getDisplayMetrics().heightPixels;
        for (int pass = 0; pass < 2; pass++) {
            decor.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY));
            decor.layout(0, 0, width, height);
        }
        int[] at = new int[2];
        backdrop.getLocationInWindow(at);
        assertEquals(0, at[0]);
        assertEquals(0, at[1]);
        assertEquals(width, backdrop.getWidth());
        assertEquals(height, backdrop.getHeight());
    }

    @Test
    public void theWindowCanLetGoOfItsBackgroundOnceTheBackdropCoversIt() {
        int[] covered = {0};
        backdrop.setOnCovered(() -> covered[0]++);
        backdrop.focus("a", cover(), false);
        pass(SETTLE_AND_FADE);
        backdrop.focus("b", cover(), false);
        pass(SETTLE_AND_FADE);
        assertEquals("told once", 1, covered[0]);
    }

    /** A second screen's backdrop in the same window, as a game's page over Home has. */
    private BackdropView nextScreen() {
        BackdropView next = new BackdropView(controller.get());
        next.setWorker(Runnable::run);
        controller.get().addContentView(next, new FrameLayout.LayoutParams(1920, 1080));
        return next;
    }

    @Test
    public void theGamesPageOpensOnTheBackdropHomeShowedForItWithNoChange() {
        Bitmap cover = cover();
        backdrop.focus("game:pc:2", cover, false);
        pass(SETTLE_AND_FADE);
        Bitmap homes = ((BitmapDrawable) back().getDrawable()).getBitmap();

        BackdropView page = nextScreen();
        int[] covered = {0};
        page.setOnCovered(() -> covered[0]++);
        assertTrue(page.adopt("game:pc:2"));
        ImageView pageBack = (ImageView) page.getChildAt(0);
        assertSame("the same picture, at once", homes, ((BitmapDrawable) pageBack.getDrawable()).getBitmap());
        assertEquals("it covers the window from the start", 1, covered[0]);
        // The page then asks for its game's backdrop with the cover it loaded, which is Home's: nothing changes.
        page.focus("game:pc:2", cover, false);
        pass(SETTLE_AND_FADE);
        assertEquals(0, page.changeCount());
        assertSame(homes, ((BitmapDrawable) pageBack.getDrawable()).getBitmap());
    }

    @Test
    public void anotherGamesBackdropIsNotAdopted() {
        backdrop.focus("game:pc:12", cover(), false);
        pass(SETTLE_AND_FADE);
        BackdropView page = nextScreen();
        assertEquals("game 1 is not game 12", false, page.adopt("game:pc:1"));
        assertNull(((ImageView) page.getChildAt(0)).getDrawable());
        // It fades in its own game's backdrop instead.
        page.focus("game:pc:1", cover(), false);
        pass(SETTLE_AND_FADE);
        assertEquals(1, page.changeCount());
    }

    private static final long SETTLE_AND_FADE = BackdropView.SETTLE_MS + BackdropView.ZOOM_MS + 100;
}
