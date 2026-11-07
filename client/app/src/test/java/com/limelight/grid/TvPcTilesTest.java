package com.limelight.grid;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.drawable.BitmapDrawable;
import android.preference.PreferenceManager;
import android.provider.Settings;
import android.util.TypedValue;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.ImageView;
import android.widget.TextView;

import com.limelight.PcView;
import com.limelight.R;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.AdaptiveGridView;
import com.limelight.ui.Motion;
import com.limelight.ui.MotionTest;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;

import java.io.File;
import java.io.FileOutputStream;

/** The PCs screen on TV: fixed-size tiles in a row from the page margin, their focus, their state and the game they are running. */
@RunWith(RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 30, application = Application.class, qualifiers = "w960dp-h540dp-land-television-xhdpi",
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class TvPcTilesTest {
    private ActivityController<MotionTest.Screen> controller;

    @Before
    public void setUp() {
        Settings.Global.putFloat(RuntimeEnvironment.getApplication().getContentResolver(),
                Settings.Global.ANIMATOR_DURATION_SCALE, 1f);
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit().clear().commit();
        TileBackdrops.clear();
        TileBackdrops.setWorker(Runnable::run);
    }

    @After
    public void tearDown() {
        TileBackdrops.setWorker(null);
        TileBackdrops.clear();
        if (controller != null) controller.close();
        RuntimeEnvironment.setFontScale(1f);
    }

    private MotionTest.Screen screen() {
        controller = Robolectric.buildActivity(MotionTest.Screen.class).setup().visible();
        return controller.get();
    }

    private static int attr(MotionTest.Screen screen, int attr) {
        TypedValue value = new TypedValue();
        assertTrue(screen.getTheme().resolveAttribute(attr, value, true));
        return value.resourceId;
    }

    private static PcView.ComputerObject pc(String uuid, ComputerDetails.State state, PairingManager.PairState pairing, NvApp running) {
        ComputerDetails details = new ComputerDetails();
        details.uuid = uuid;
        details.name = "PC " + uuid;
        details.state = state;
        details.pairState = pairing;
        if (running != null) details.runningGameId = running.getAppId();
        PcView.ComputerObject computer = new PcView.ComputerObject(details);
        computer.runningGame = running;
        return computer;
    }

    @Test
    public void aFocusedTileGrowsByFivePercentAndLifts() {
        MotionTest.Screen screen = screen();
        assertEquals(R.animator.tv_tile_focus, attr(screen, R.attr.nightjarTileFocusAnimator));
        assertEquals(R.drawable.tv_tile_ring, attr(screen, R.attr.nightjarTileRing));
        View tile = screen.getLayoutInflater().inflate(R.layout.pc_grid_item, new FrameLayout(screen), false);
        tile.setSelected(true);
        tile.getStateListAnimator().jumpToCurrentState();
        assertEquals(1.05f, tile.getScaleX(), 0.001f);
        assertEquals(1.05f, tile.getScaleY(), 0.001f);
        assertTrue(tile.getTranslationZ() > 0);
        // The ring and the glow ride on the tile's state.
        View ring = tile.findViewById(R.id.grid_ring);
        assertTrue(ring.isDuplicateParentStateEnabled());
        assertNotNull(ring.getBackground());
        assertTrue(tile.findViewById(R.id.grid_glow).isDuplicateParentStateEnabled());
    }

    @Test
    public void withReducedMotionATileStaysPutAndItsRingShowsAtOnce() {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putString(Motion.REDUCE_MOTION, Motion.REDUCE).commit();
        MotionTest.Screen screen = screen();
        assertEquals(R.animator.tv_poster_focus_static, attr(screen, R.attr.nightjarTileFocusAnimator));
        assertEquals(R.drawable.tv_tile_ring_static, attr(screen, R.attr.nightjarTileRing));
        View tile = screen.getLayoutInflater().inflate(R.layout.pc_grid_item, new FrameLayout(screen), false);
        tile.setSelected(true);
        tile.getStateListAnimator().jumpToCurrentState();
        assertEquals(1f, tile.getScaleX(), 0f);
    }

    @Test
    public void highContrastThickensTheTilesRing() {
        PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication()).edit()
                .putBoolean(com.limelight.ui.AppTheme.HIGH_CONTRAST, true).commit();
        assertEquals(R.drawable.tv_tile_ring_hc, attr(screen(), R.attr.nightjarTileRing));
    }

    @Test
    public void eachStateHasItsDotAndItsBackground() {
        assertEquals(R.drawable.tv_dot_live, PcGridAdapter.dot(PcGridAdapter.RowState.RUNNING));
        assertEquals(R.drawable.tv_dot_live, PcGridAdapter.dot(PcGridAdapter.RowState.READY));
        assertEquals(R.drawable.tv_dot_suspended, PcGridAdapter.dot(PcGridAdapter.RowState.NOT_PAIRED));
        assertEquals(R.drawable.tv_dot_suspended, PcGridAdapter.dot(PcGridAdapter.RowState.WAKING));
        assertEquals(R.drawable.tv_dot_idle, PcGridAdapter.dot(PcGridAdapter.RowState.OFFLINE));
        // Phone rows: the theme's ok dot for a PC that is up, the same neutral dot and ring otherwise.
        assertEquals(R.drawable.status_dot_ok, PcGridAdapter.phoneDot(PcGridAdapter.RowState.RUNNING));
        assertEquals(R.drawable.status_dot_ok, PcGridAdapter.phoneDot(PcGridAdapter.RowState.READY));
        assertEquals(R.drawable.tv_dot_suspended, PcGridAdapter.phoneDot(PcGridAdapter.RowState.CHECKING));
        assertEquals(R.drawable.tv_dot_idle, PcGridAdapter.phoneDot(PcGridAdapter.RowState.OFFLINE));
    }

    @Test
    public void aPcsWordsSayItsStateAndItsOneAction() {
        MotionTest.Screen screen = screen();
        NvApp game = new NvApp("Racing Game", 2, false);
        PcView.ComputerObject playing = pc("a", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, game);
        assertEquals("Racing Game", PcGridAdapter.shortStatus(screen, playing));
        assertEquals(R.string.applist_menu_resume, PcGridAdapter.actionWord(screen, playing));
        PcView.ComputerObject ready = pc("b", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, null);
        assertEquals(screen.getString(R.string.pc_status_online), PcGridAdapter.shortStatus(screen, ready));
        assertEquals(R.string.console_hint_open, PcGridAdapter.actionWord(screen, ready));
        PcView.ComputerObject locked = pc("c", ComputerDetails.State.ONLINE, PairingManager.PairState.NOT_PAIRED, null);
        assertEquals(screen.getString(R.string.pc_status_not_paired), PcGridAdapter.shortStatus(screen, locked));
        assertEquals(R.string.pcview_menu_pair_pc, PcGridAdapter.actionWord(screen, locked));
        // Away with no address saved: no Wake to promise.
        PcView.ComputerObject away = pc("d", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED, null);
        assertEquals(screen.getString(R.string.pc_offline), PcGridAdapter.shortStatus(screen, away));
        assertEquals(0, PcGridAdapter.actionWord(screen, away));
    }

    @Test
    public void aSelectedCardsGlowLeavesTheCardItselfAlone() {
        MotionTest.Screen screen = screen();
        View tile = screen.getLayoutInflater().inflate(R.layout.pc_grid_item, new FrameLayout(screen), false);
        View glow = tile.findViewById(R.id.grid_glow);
        assertTrue(glow instanceof com.limelight.ui.OutsideGlow);
        int reach = -screen.getResources().getDimensionPixelSize(R.dimen.tile_glow_back);
        glow.setBackground(new android.graphics.drawable.ColorDrawable(0xFFFFFFFF));
        glow.measure(View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(300, View.MeasureSpec.EXACTLY));
        glow.layout(0, 0, 400, 300);
        Bitmap drawn = Bitmap.createBitmap(400, 300, Bitmap.Config.ARGB_8888);
        glow.draw(new android.graphics.Canvas(drawn));
        assertEquals("the glow around the card", 0xFFFFFFFF, drawn.getPixel(reach / 2, 150));
        assertEquals("nothing inside it", 0, drawn.getPixel(200, 150));
    }

    private static View bind(MotionTest.Screen screen, PcGridAdapter adapter, int position) {
        return adapter.getView(position, null, new FrameLayout(screen));
    }

    @Test
    public void aPcPlayingAGameShowsItsCoverOnTheScreenAndTheOthersNone() throws Exception {
        MotionTest.Screen screen = screen();
        NvApp game = new NvApp("Racing Game", 2, false);
        writeCover(screen, "live", game, 0xFFCC4422);

        PcGridAdapter adapter = new PcGridAdapter(screen, PreferenceConfiguration.readPreferences(screen));
        adapter.addComputer(pc("live", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, game));
        adapter.addComputer(pc("away", ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED, null));
        View playing = bind(screen, adapter, 0);
        ShadowLooper.idleMainLooper();
        ImageView backdrop = playing.findViewById(R.id.grid_backdrop);
        assertTrue(backdrop.getDrawable() instanceof BitmapDrawable);
        Bitmap shown = ((BitmapDrawable) backdrop.getDrawable()).getBitmap();
        assertTrue("cut for the monitor's wide face", shown.getWidth() > shown.getHeight());
        assertEquals("the cover's own colour, not darkened", 0xFFCC4422, shown.getPixel(shown.getWidth() / 2, shown.getHeight() / 2));
        TextView status = playing.findViewById(R.id.grid_status);
        assertNotNull("the dot", status.getCompoundDrawablesRelative()[0]);
        // Bound again from the cache: still the cover alone.
        ImageView again = bind(screen, adapter, 0).findViewById(R.id.grid_backdrop);
        assertTrue(again.getDrawable() instanceof BitmapDrawable);

        View away = bind(screen, adapter, 1);
        assertNull(((ImageView) away.findViewById(R.id.grid_backdrop)).getDrawable());
        assertEquals("a tile keeps its name at the foot when no PC has an update",
                View.GONE, away.findViewById(R.id.grid_update).getVisibility());
    }

    @Test
    public void aRunningGameWithNoCoverOnDiskKeepsTheGradient() {
        MotionTest.Screen screen = screen();
        PcGridAdapter adapter = new PcGridAdapter(screen, PreferenceConfiguration.readPreferences(screen));
        adapter.addComputer(pc("bare", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, new NvApp("Game", 5, false)));
        View tile = bind(screen, adapter, 0);
        ShadowLooper.idleMainLooper();
        ImageView backdrop = tile.findViewById(R.id.grid_backdrop);
        assertNull(backdrop.getDrawable());
    }

    @Test
    public void aRunningGameWhoseCoverIsTheHostsPlaceholderKeepsTheGradient() throws Exception {
        MotionTest.Screen screen = screen();
        NvApp game = new NvApp("Coverless Game", 6, false);
        File file = new DiskAssetLoader(screen).getFile("grey", game.getAssetCacheKey());
        assertTrue(file.getParentFile().isDirectory() || file.getParentFile().mkdirs());
        java.nio.file.Files.copy(new File("../protocol/client/tests/fixtures/box-placeholder.png").toPath(), file.toPath(),
                java.nio.file.StandardCopyOption.REPLACE_EXISTING);
        PcGridAdapter adapter = new PcGridAdapter(screen, PreferenceConfiguration.readPreferences(screen));
        adapter.addComputer(pc("grey", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, game));
        View tile = bind(screen, adapter, 0);
        ShadowLooper.idleMainLooper();
        ImageView backdrop = tile.findViewById(R.id.grid_backdrop);
        assertNull("no grey box", backdrop.getDrawable());
    }

    private static void writeCover(MotionTest.Screen screen, String uuid, NvApp game, int color) throws Exception {
        File file = new DiskAssetLoader(screen).getFile(uuid, game.getAssetCacheKey());
        assertTrue(file.getParentFile().isDirectory() || file.getParentFile().mkdirs());
        Bitmap cover = Bitmap.createBitmap(60, 90, Bitmap.Config.ARGB_8888);
        cover.eraseColor(color);
        try (FileOutputStream out = new FileOutputStream(file)) {
            cover.compress(Bitmap.CompressFormat.PNG, 100, out);
        }
    }

    @Test
    public void aCoverFetchedAfterAMissShowsOnALaterBind() throws Exception {
        MotionTest.Screen screen = screen();
        NvApp game = new NvApp("Late Game", 7, false);
        PcGridAdapter adapter = new PcGridAdapter(screen, PreferenceConfiguration.readPreferences(screen));
        adapter.addComputer(pc("late", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, game));
        View tile = adapter.getView(0, null, new FrameLayout(screen));
        ShadowLooper.idleMainLooper();
        assertNull("not on disk yet", ((ImageView) tile.findViewById(R.id.grid_backdrop)).getDrawable());

        writeCover(screen, "late", game, 0xFF2244CC);
        // A rebind straight away does not hit the disk again...
        tile = adapter.getView(0, tile, new FrameLayout(screen));
        ShadowLooper.idleMainLooper();
        assertNull(((ImageView) tile.findViewById(R.id.grid_backdrop)).getDrawable());
        // ...a later one does, and finds it.
        ShadowLooper.idleMainLooper(TileBackdrops.RETRY_MS, java.util.concurrent.TimeUnit.MILLISECONDS);
        tile = adapter.getView(0, tile, new FrameLayout(screen));
        ShadowLooper.idleMainLooper();
        ImageView backdrop = tile.findViewById(R.id.grid_backdrop);
        assertTrue(backdrop.getDrawable() instanceof BitmapDrawable);
    }

    @Test
    public void aNewPcsScreenLooksForAMissingCoverAgain() throws Exception {
        MotionTest.Screen screen = screen();
        NvApp game = new NvApp("Late Game", 8, false);
        PcGridAdapter first = new PcGridAdapter(screen, PreferenceConfiguration.readPreferences(screen));
        first.addComputer(pc("again", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, game));
        bind(screen, first, 0);
        ShadowLooper.idleMainLooper();
        writeCover(screen, "again", game, 0xFF22CC44);
        PcGridAdapter second = new PcGridAdapter(screen, PreferenceConfiguration.readPreferences(screen));
        second.addComputer(pc("again", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, game));
        View tile = bind(screen, second, 0);
        ShadowLooper.idleMainLooper();
        assertTrue(((ImageView) tile.findViewById(R.id.grid_backdrop)).getDrawable() instanceof BitmapDrawable);
    }

    /** Lays a tile out as the grid does: its cell's width, as tall as it wants. */
    private static void layOutTile(View tile, MotionTest.Screen screen) {
        int width = screen.getResources().getDimensionPixelSize(R.dimen.tile_cell);
        tile.measure(View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED));
        tile.layout(0, 0, tile.getMeasuredWidth(), tile.getMeasuredHeight());
    }

    private static android.graphics.Rect rectIn(ViewGroup ancestor, View view) {
        android.graphics.Rect rect = new android.graphics.Rect(0, 0, view.getWidth(), view.getHeight());
        ancestor.offsetDescendantRectToMyCoords(view, rect);
        return rect;
    }

    /** The cover is on the monitor's face now, so the name never has to read over a white one. */
    @Test
    public void aTilesNameSitsUnderTheCoverNotOnIt() throws Exception {
        MotionTest.Screen screen = screen();
        NvApp game = new NvApp("Snow Game", 3, false);
        writeCover(screen, "white", game, 0xFFFFFFFF);
        PcGridAdapter adapter = new PcGridAdapter(screen, PreferenceConfiguration.readPreferences(screen));
        adapter.addComputer(pc("white", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, game));
        ViewGroup tile = (ViewGroup) bind(screen, adapter, 0);
        ShadowLooper.idleMainLooper();
        assertTrue(((ImageView) tile.findViewById(R.id.grid_backdrop)).getDrawable() instanceof BitmapDrawable);
        layOutTile(tile, screen);
        assertTrue(rectIn(tile, tile.findViewById(R.id.grid_text)).top >= rectIn(tile, tile.findViewById(R.id.grid_screen)).bottom);
    }

    private void textStaysClearOfTheGlyph(float fontScale) {
        RuntimeEnvironment.setFontScale(fontScale);
        MotionTest.Screen screen = screen();
        PcGridAdapter adapter = new PcGridAdapter(screen, PreferenceConfiguration.readPreferences(screen));
        adapter.addComputer(pc("big", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, null));
        ViewGroup tile = (ViewGroup) bind(screen, adapter, 0);
        TextView update = tile.findViewById(R.id.grid_update);
        update.setText("Update 3.1 available");
        update.setVisibility(View.VISIBLE);
        layOutTile(tile, screen);
        android.graphics.Rect glyph = rectIn(tile, (View) tile.findViewById(R.id.grid_image).getParent());
        android.graphics.Rect name = rectIn(tile, tile.findViewById(R.id.grid_text));
        assertTrue("at " + fontScale + " the name (" + name.top + ") stays below the glyph (" + glyph.bottom + ")",
                name.top >= glyph.bottom);
    }

    @Test
    public void atALargerFontTheTileGrowsSoItsTextStaysClearOfTheGlyph() {
        textStaysClearOfTheGlyph(1.3f);
    }

    @Test
    public void atTheLargestFontTheTileGrowsSoItsTextStaysClearOfTheGlyph() {
        textStaysClearOfTheGlyph(1.5f);
    }

    @Test
    public void atTheDefaultFontTheTileKeepsItsSize() {
        textStaysClearOfTheGlyph(1f);
        MotionTest.Screen screen = controller.get();
        assertEquals(screen.getResources().getDimensionPixelSize(R.dimen.tile_height),
                PcGridAdapter.tileHeight(screen, R.dimen.tile_height));
    }

    private static AdaptiveGridView grid(MotionTest.Screen screen, int pcs) {
        AdaptiveGridView grid = (AdaptiveGridView) screen.getLayoutInflater().inflate(R.layout.pc_grid_view,
                (ViewGroup) screen.findViewById(android.R.id.content), false);
        PcGridAdapter adapter = new PcGridAdapter(screen, PreferenceConfiguration.readPreferences(screen));
        for (int i = 0; i < pcs; i++) {
            adapter.addComputer(pc("pc" + i, ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, null));
        }
        grid.setAdapter(adapter);
        grid.setColumnPolicy(true, false);
        return grid;
    }

    /** The visible colour a selector draws for a focused, selected item: 0 when it draws nothing. */
    private static int focusedSelectorAlpha(android.graphics.drawable.Drawable selector) {
        selector.setState(new int[]{android.R.attr.state_focused, android.R.attr.state_selected, android.R.attr.state_enabled});
        android.graphics.drawable.Drawable shown = selector.getCurrent();
        if (shown instanceof android.graphics.drawable.ColorDrawable) {
            return ((android.graphics.drawable.ColorDrawable) shown).getAlpha();
        }
        return shown == null ? 0 : 255;
    }

    @Test
    public void theGridsDrawNoSecondRingOverTheTiles() {
        MotionTest.Screen screen = screen();
        assertEquals(0, focusedSelectorAlpha(grid(screen, 2).getSelector()));
        android.widget.GridView apps = (android.widget.GridView) screen.getLayoutInflater().inflate(R.layout.app_grid_view,
                (ViewGroup) screen.findViewById(android.R.id.content), false);
        assertEquals(0, focusedSelectorAlpha(apps.getSelector()));
    }

    /** The width a 1080p TV leaves inside its overscan: 864dp, at xhdpi. */
    private static final int CONTENT = 864 * 2;
    private static final int CELL = 212 * 2;

    private static void measure(View grid) {
        grid.measure(View.MeasureSpec.makeMeasureSpec(CONTENT, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(900, View.MeasureSpec.EXACTLY));
    }

    /** The row starts at the page's margin with the title, however many PCs there are: as many columns as fit, no padding added. */
    @Test
    public void pcsStartAtTheMarginWhateverTheirCount() {
        for (int count : new int[] {1, 2, 5}) {
            AdaptiveGridView grid = grid(screen(), count);
            measure(grid);
            assertEquals(4, grid.getNumColumns());
            assertEquals(0, grid.getPaddingLeft());
            assertEquals(0, grid.getPaddingRight());
            controller.close();
            controller = null;
        }
    }
}
