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
        File file = new DiskAssetLoader(screen).getFile("live", game.getAssetCacheKey());
        assertTrue(file.getParentFile().isDirectory() || file.getParentFile().mkdirs());
        Bitmap cover = Bitmap.createBitmap(60, 90, Bitmap.Config.ARGB_8888);
        cover.eraseColor(0xFFCC4422);
        try (FileOutputStream out = new FileOutputStream(file)) {
            cover.compress(Bitmap.CompressFormat.PNG, 100, out);
        }

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
        TextView status = playing.findViewById(R.id.grid_status);
        assertNotNull("the dot", status.getCompoundDrawablesRelative()[0]);

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
