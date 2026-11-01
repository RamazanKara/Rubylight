package com.limelight.console;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

/** Home's view: each device's own look until one is chosen, what is stored, and the columns per card size. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HomeViewTest {
    private SharedPreferences prefs;

    @Before
    public void setUp() {
        prefs = PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
        prefs.edit().clear().commit();
    }

    @After
    public void tearDown() {
        prefs.edit().clear().commit();
    }

    @Test
    public void nothingStoredKeepsEachDevicesLook() {
        assertEquals(new HomeView(HomeView.Layout.GRID, HomeView.Group.NONE, HomeView.Size.MEDIUM),
                HomeView.read(prefs, false));
        assertEquals(new HomeView(HomeView.Layout.ROWS, HomeView.Group.STORE, HomeView.Size.MEDIUM),
                HomeView.read(prefs, true));
        assertFalse(HomeView.defaults(false).grouped());
        assertTrue(HomeView.defaults(true).grouped());
    }

    @Test
    public void aSavedViewReadsBackOnEitherDevice() {
        HomeView chosen = new HomeView(HomeView.Layout.GRID, HomeView.Group.STORE, HomeView.Size.LARGE);
        chosen.save(prefs);
        assertEquals("grid", prefs.getString("home_view_layout", null));
        assertEquals("store", prefs.getString("home_view_group", null));
        assertEquals("large", prefs.getString("home_view_size", null));
        assertEquals(chosen, HomeView.read(prefs, true));
        assertEquals(chosen, HomeView.read(prefs, false));
    }

    @Test
    public void aValueThisVersionDoesNotKnowKeepsTheDefault() {
        prefs.edit().putString("home_view_layout", "carousel").putString("home_view_size", "SMALL").commit();
        HomeView view = HomeView.read(prefs, true);
        assertEquals(HomeView.Layout.ROWS, view.layout);
        assertEquals(HomeView.Group.STORE, view.group);
        assertEquals(HomeView.Size.SMALL, view.size);
    }

    @Test
    public void onePartChangesAtATime() {
        HomeView tv = HomeView.defaults(true);
        assertEquals(new HomeView(HomeView.Layout.GRID, HomeView.Group.STORE, HomeView.Size.MEDIUM),
                tv.with(HomeView.Layout.GRID));
        assertEquals(new HomeView(HomeView.Layout.ROWS, HomeView.Group.NONE, HomeView.Size.MEDIUM),
                tv.with(HomeView.Group.NONE));
        assertEquals(new HomeView(HomeView.Layout.ROWS, HomeView.Group.STORE, HomeView.Size.SMALL),
                tv.with(HomeView.Size.SMALL));
        assertEquals(tv, tv.with(HomeView.Size.MEDIUM));
    }

    @Test
    public void smallerCardsFitMoreColumns() {
        // A 411 dp phone: three columns as before at the medium size.
        assertEquals(ConsoleHomeActivity.columnsFor(411), ConsoleHomeActivity.columnsFor(411, HomeView.Size.MEDIUM));
        assertEquals(3, ConsoleHomeActivity.columnsFor(411, HomeView.Size.MEDIUM));
        assertEquals(4, ConsoleHomeActivity.columnsFor(411, HomeView.Size.SMALL));
        assertEquals(2, ConsoleHomeActivity.columnsFor(411, HomeView.Size.LARGE));
        // A tablet keeps an even count from four on, so wide cards still pair up.
        assertEquals(6, ConsoleHomeActivity.columnsFor(900, HomeView.Size.MEDIUM));
        assertEquals(8, ConsoleHomeActivity.columnsFor(900, HomeView.Size.SMALL));
        assertEquals(4, ConsoleHomeActivity.columnsFor(900, HomeView.Size.LARGE));
    }

    @Test
    public void aTvGridFitsAsManyPostersAsTheSizeAllowsAndAtLeastTwo() {
        assertEquals(8, ConsoleHomeActivity.tvColumnsFor(1800, 220));
        assertEquals(6, ConsoleHomeActivity.tvColumnsFor(1800, 286));
        assertEquals(2, ConsoleHomeActivity.tvColumnsFor(300, 286));
    }
}
