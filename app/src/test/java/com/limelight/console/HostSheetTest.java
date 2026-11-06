package com.limelight.console;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.TextView;

import androidx.appcompat.app.AppCompatActivity;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.ui.GlassHost;
import com.limelight.ui.GlassSurface;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/** The PCs behind Home's capsule: glass on the page, a dot and a word per state, a mark on the current one. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = android.app.Application.class)
public class HostSheetTest {
    public static final class Screen extends AppCompatActivity {
        @Override protected void onCreate(android.os.Bundle saved) {
            setTheme(R.style.AppTheme);
            super.onCreate(saved);
        }
    }

    private static HomeModel.HostCard card(String name, boolean online) {
        return new HomeModel.HostCard("u:" + name, name, online ? HomeModel.HostLine.GAMES : HomeModel.HostLine.OFFLINE, 3, online);
    }

    private static ComputerDetails details(ComputerDetails.State state, PairingManager.PairState pair, int game) {
        ComputerDetails details = new ComputerDetails();
        details.state = state;
        details.pairState = pair;
        details.runningGameId = game;
        return details;
    }

    private HostSheet open(ActivityController<Screen> controller, View anchor, List<HostSheet.Row> rows, List<HomeModel.HostCard> picked) {
        GlassHost host = controller.get().findViewById(android.R.id.content).findViewWithTag("host");
        HostSheet sheet = HostSheet.attach(controller.get(), host, anchor);
        sheet.show(rows, picked::add);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        return sheet;
    }

    private ActivityController<Screen> screen(View[] anchor) {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        GlassHost host = new GlassHost(controller.get());
        host.setTag("host");
        host.addView(new FrameLayout(controller.get()));
        anchor[0] = new View(controller.get());
        host.addView(anchor[0], new FrameLayout.LayoutParams(100, 100));
        controller.get().setContentView(host);
        return controller;
    }

    @Test
    public void eachStateHasItsDotAndItsWords() {
        Screen context = Robolectric.buildActivity(Screen.class).setup().get();
        HostSheet.Row running = HostSheet.rowOf(context, card("A", true), details(ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, 2), "Racing Game", true);
        assertEquals("Racing Game", running.status);
        assertEquals(R.drawable.status_dot_ok, running.dot);
        HostSheet.Row online = HostSheet.rowOf(context, card("B", true), details(ComputerDetails.State.ONLINE, PairingManager.PairState.PAIRED, 0), null, false);
        assertEquals(context.getString(R.string.pc_status_online), online.status);
        assertEquals(R.drawable.status_dot_ok, online.dot);
        HostSheet.Row unpaired = HostSheet.rowOf(context, card("C", true), details(ComputerDetails.State.ONLINE, PairingManager.PairState.NOT_PAIRED, 0), null, false);
        assertEquals(context.getString(R.string.pc_status_not_paired), unpaired.status);
        HostSheet.Row offline = HostSheet.rowOf(context, card("D", false), details(ComputerDetails.State.OFFLINE, PairingManager.PairState.PAIRED, 0), null, false);
        assertEquals(context.getString(R.string.pc_offline), offline.status);
        assertEquals(R.drawable.tv_dot_idle, offline.dot);
        assertEquals("not known yet, it says what the card knows", context.getString(R.string.pc_offline),
                HostSheet.rowOf(context, card("E", false), null, null, false).status);
    }

    @Test
    public void itIsGlassOnThePageMarksTheCurrentPcAndTellsWhichWasPicked() {
        View[] anchor = new View[1];
        ActivityController<Screen> controller = screen(anchor);
        List<HomeModel.HostCard> picked = new ArrayList<>();
        HomeModel.HostCard a = card("A", true);
        HomeModel.HostCard b = card("B", false);
        HostSheet sheet = open(controller, anchor[0], Arrays.asList(
                new HostSheet.Row(a, "Online", R.drawable.status_dot_ok, true),
                new HostSheet.Row(b, "Offline", R.drawable.tv_dot_idle, false)), picked);

        assertTrue(sheet.isOpen());
        assertTrue("its panel is glass in the activity's host", sheet.findViewById(R.id.host_sheet_panel) instanceof GlassSurface);
        assertFalse("a clipped shadow would show as a square behind the rounded corners", sheet.getClipChildren());
        ViewGroup rows = sheet.findViewById(R.id.host_sheet_rows);
        assertEquals(2, rows.getChildCount());
        assertEquals(View.VISIBLE, rows.getChildAt(0).findViewById(R.id.host_row_mark).getVisibility());
        assertEquals(View.GONE, rows.getChildAt(1).findViewById(R.id.host_row_mark).getVisibility());
        assertEquals("B", ((TextView) rows.getChildAt(1).findViewById(R.id.host_row_name)).getText().toString());

        rows.getChildAt(1).performClick();
        assertEquals(1, picked.size());
        assertEquals(b, picked.get(0));
        assertFalse(sheet.isOpen());
    }

    @Test
    public void backClosesItAndATapOutsideToo() {
        View[] anchor = new View[1];
        ActivityController<Screen> controller = screen(anchor);
        List<HomeModel.HostCard> picked = new ArrayList<>();
        List<HostSheet.Row> rows = Arrays.asList(new HostSheet.Row(card("A", true), "Online", R.drawable.status_dot_ok, true));
        HostSheet sheet = open(controller, anchor[0], rows, picked);

        controller.get().getOnBackPressedDispatcher().onBackPressed();
        assertFalse(sheet.isOpen());

        sheet.show(rows, picked::add);
        assertTrue(sheet.isOpen());
        sheet.findViewById(R.id.host_sheet_scrim).performClick();
        assertFalse(sheet.isOpen());
        assertTrue("nothing was picked", picked.isEmpty());
    }
}
