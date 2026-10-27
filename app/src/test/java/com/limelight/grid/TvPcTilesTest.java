package com.limelight.grid;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;
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

/** The PCs screen on TV: fixed-size tiles in a centred row, their focus, their state and the game they are running. */
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
        assertEquals(R.drawable.tv_tile_bg_live, TileBackdrops.gradient(PcGridAdapter.RowState.READY));
        assertEquals(R.drawable.tv_tile_bg_attention, TileBackdrops.gradient(PcGridAdapter.RowState.NOT_PAIRED));
        assertEquals(R.drawable.tv_tile_bg_idle, TileBackdrops.gradient(PcGridAdapter.RowState.OFFLINE));
    }

    private static View bind(MotionTest.Screen screen, PcGridAdapter adapter, int position) {
        return adapter.getView(position, null, new FrameLayout(screen));
    }

    @Test
    public void aPcPlayingAGameShowsItsBlurredCoverAndTheOthersTheirGradient() throws Exception {
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
        assertEquals(96, shown.getWidth());
        assertEquals(54, shown.getHeight());
        int pixel = shown.getPixel(48, 27);
        assertTrue("the cover's red, darkened", ((pixel >> 16) & 0xFF) > (pixel & 0xFF));
        assertTrue("darkened", ((pixel >> 16) & 0xFF) < 0xCC);
        assertNull("the opaque cover replaces the gradient, no overdraw", backdrop.getBackground());
        TextView status = playing.findViewById(R.id.grid_status);
        assertNotNull("the dot", status.getCompoundDrawablesRelative()[0]);
        // Bound again from the cache: still the cover alone.
        ImageView again = bind(screen, adapter, 0).findViewById(R.id.grid_backdrop);
        assertTrue(again.getDrawable() instanceof BitmapDrawable);
        assertNull(again.getBackground());

        View away = bind(screen, adapter, 1);
        assertNull(((ImageView) away.findViewById(R.id.grid_backdrop)).getDrawable());
        assertNotNull(away.findViewById(R.id.grid_backdrop).getBackground());
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
        assertNotNull(backdrop.getBackground());
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
        assertNull(backdrop.getBackground());
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

    private static double luminance(int color) {
        double[] c = {((color >> 16) & 0xFF) / 255.0, ((color >> 8) & 0xFF) / 255.0, (color & 0xFF) / 255.0};
        for (int i = 0; i < 3; i++) c[i] = c[i] <= 0.03928 ? c[i] / 12.92 : Math.pow((c[i] + 0.055) / 1.055, 2.4);
        return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    }

    @Test
    @Config(qualifiers = "+night")
    public void aTilesNameReadsAtFourAndAHalfToOneOverAWhiteCover() throws Exception {
        MotionTest.Screen screen = screen();
        NvApp game = new NvApp("Snow Game", 3, false);
        writeCover(screen, "white", game, 0xFFFFFFFF);
        PcGridAdapter adapter = new PcGridAdapter(screen, PreferenceConfiguration.readPreferences(screen));
        adapter.addComputer(pc("white", ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, game));
        ViewGroup tile = (ViewGroup) bind(screen, adapter, 0);
        ShadowLooper.idleMainLooper();
        assertTrue(((ImageView) tile.findViewById(R.id.grid_backdrop)).getDrawable() instanceof BitmapDrawable);
        layOutTile(tile, screen);
        Bitmap drawn = Bitmap.createBitmap(tile.getWidth(), tile.getHeight(), Bitmap.Config.ARGB_8888);
        tile.draw(new android.graphics.Canvas(drawn));

        TextView name = tile.findViewById(R.id.grid_text);
        android.graphics.Rect rect = rectIn(tile, name);
        // The name's line, right of the glyphs: the brightest the background gets behind the text.
        int textEnd = rect.left + name.getPaddingStart() + Math.round(name.getPaint().measureText(name.getText().toString()));
        double background = 0;
        for (int y = rect.top; y < rect.bottom; y++) {
            for (int x = Math.max(textEnd + 4, rect.right - rect.width() / 3); x < rect.right; x++) {
                background = Math.max(background, luminance(drawn.getPixel(x, y)));
            }
        }
        assertTrue("the TV's dark theme: light text", luminance(name.getCurrentTextColor()) > 0.7);
        double text = luminance(name.getCurrentTextColor());
        double ratio = (Math.max(text, background) + 0.05) / (Math.min(text, background) + 0.05);
        assertTrue("name contrast " + ratio, ratio >= 4.5);
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
        android.graphics.Rect scrim = rectIn(tile, tile.findViewById(R.id.grid_scrim));
        assertTrue("and the scrim reaches above the name", scrim.top <= name.top);
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

    @Test
    public void paddingSetAfterTheFirstMeasureBecomesTheNewBase() {
        AdaptiveGridView grid = grid(screen(), 2);
        measure(grid);
        int extra = 40;
        grid.setPadding(extra, grid.getPaddingTop(), extra, grid.getPaddingBottom());
        measure(grid);
        assertEquals(2, grid.getNumColumns());
        assertEquals(extra + (CONTENT - 2 * extra - 2 * CELL) / 2, grid.getPaddingLeft());
        assertEquals(grid.getPaddingLeft(), grid.getPaddingRight());
        grid.setPaddingRelative(0, grid.getPaddingTop(), 0, grid.getPaddingBottom());
        measure(grid);
        assertEquals((CONTENT - 2 * CELL) / 2, grid.getPaddingLeft());
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
    private static final int CELL = 280 * 2;

    private static void measure(View grid) {
        grid.measure(View.MeasureSpec.makeMeasureSpec(CONTENT, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(900, View.MeasureSpec.EXACTLY));
    }

    @Test
    public void twoPcsSitAsACentredPair() {
        AdaptiveGridView grid = grid(screen(), 2);
        measure(grid);
        assertEquals(2, grid.getNumColumns());
        assertEquals((CONTENT - 2 * CELL) / 2, grid.getPaddingLeft());
        assertEquals(grid.getPaddingLeft(), grid.getPaddingRight());
    }

    @Test
    public void onePcSitsInTheMiddle() {
        AdaptiveGridView grid = grid(screen(), 1);
        measure(grid);
        assertEquals(1, grid.getNumColumns());
        assertEquals((CONTENT - CELL) / 2, grid.getPaddingLeft());
    }

    @Test
    public void morePcsFillThreeColumnsOfACentredBlockAndTheBlockFollowsTheCount() {
        AdaptiveGridView grid = grid(screen(), 5);
        measure(grid);
        assertEquals(3, grid.getNumColumns());
        assertEquals((CONTENT - 3 * CELL) / 2, grid.getPaddingLeft());
        // Down to two PCs: the padding comes from the layout's own, not from the last centring.
        PcGridAdapter adapter = (PcGridAdapter) grid.getAdapter();
        adapter.removeComputer((PcView.ComputerObject) adapter.getItem(0));
        adapter.removeComputer((PcView.ComputerObject) adapter.getItem(0));
        adapter.removeComputer((PcView.ComputerObject) adapter.getItem(0));
        adapter.notifyDataSetChanged();
        measure(grid);
        assertEquals(2, grid.getNumColumns());
        assertEquals((CONTENT - 2 * CELL) / 2, grid.getPaddingLeft());
        assertNotEquals(0, grid.getPaddingLeft());
    }
}
