package com.limelight.ui;

import android.app.Activity;
import android.app.Application;
import android.graphics.Rect;
import android.view.Gravity;
import android.view.View;
import android.widget.FrameLayout;

import androidx.core.graphics.Insets;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class OverlayPlacementTest {
    // A 2400x1080 landscape phone; the overlay is the one-line compact text.
    private static final Rect AREA = new Rect(16, 16, 2384, 1064);
    private static final int WIDTH = 520, HEIGHT = 56;

    @Test
    public void withoutControlsTheOverlayStaysTopStart() {
        assertEquals(OverlayPlacement.Spot.TOP_START,
                OverlayPlacement.choose(AREA, WIDTH, HEIGHT, Collections.emptyList(), false));
        assertEquals(new Rect(16, 16, 536, 72),
                OverlayPlacement.bounds(OverlayPlacement.Spot.TOP_START, AREA, WIDTH, HEIGHT, false));
    }

    @Test
    public void eachPreferredCornerIsKeptWhenNothingCoversIt() {
        OverlayPlacement.Spot[] ltr = {OverlayPlacement.Spot.TOP_START, OverlayPlacement.Spot.TOP_END,
                OverlayPlacement.Spot.BOTTOM_START, OverlayPlacement.Spot.BOTTOM_END};
        OverlayPlacement.Corner[] corners = OverlayPlacement.Corner.values();
        for (int i = 0; i < corners.length; i++) {
            assertEquals(corners[i].toString(), ltr[i], OverlayPlacement.choose(AREA, WIDTH, HEIGHT,
                    Collections.emptyList(), corners[i].spot(false), false));
            // Left and right are the screen's own: a right-to-left layout swaps the start and end spots.
            OverlayPlacement.Spot rtl = corners[i].spot(true);
            assertEquals(corners[i].toString(), ltr[i ^ 1], rtl);
            assertEquals(corners[i].toString(), rtl, OverlayPlacement.choose(AREA, WIDTH, HEIGHT,
                    Collections.emptyList(), rtl, true));
        }
    }

    @Test
    public void cornersParseTheirStoredValuesAndUnknownOnesAreTopLeft() {
        for (OverlayPlacement.Corner corner : OverlayPlacement.Corner.values()) {
            assertEquals(corner, OverlayPlacement.Corner.parse(corner.key));
        }
        assertEquals(OverlayPlacement.Corner.TOP_LEFT, OverlayPlacement.Corner.parse(null));
        assertEquals(OverlayPlacement.Corner.TOP_LEFT, OverlayPlacement.Corner.parse("center"));
    }

    @Test
    public void aCoveredPreferredCornerFallsBackThroughTheOtherSpotsWithoutRepeating() {
        // Bottom end is wanted but a control sits there: top start, the first of the usual order, is free.
        Rect bottomEnd = OverlayPlacement.bounds(OverlayPlacement.Spot.BOTTOM_END, AREA, WIDTH, HEIGHT, false);
        assertEquals(OverlayPlacement.Spot.TOP_START, OverlayPlacement.choose(AREA, WIDTH, HEIGHT,
                Collections.singletonList(bottomEnd), OverlayPlacement.Spot.BOTTOM_END, false));
        // Every spot covered, the preferred one the least: it stays.
        List<Rect> all = new ArrayList<>();
        for (OverlayPlacement.Spot spot : OverlayPlacement.Spot.values()) {
            Rect bounds = OverlayPlacement.bounds(spot, AREA, WIDTH, HEIGHT, false);
            bounds.inset(spot == OverlayPlacement.Spot.BOTTOM_END ? 20 : 0, 0);
            all.add(bounds);
        }
        assertEquals(OverlayPlacement.Spot.BOTTOM_END, OverlayPlacement.choose(AREA, WIDTH, HEIGHT, all,
                OverlayPlacement.Spot.BOTTOM_END, false));
    }

    @Test
    public void rightCornersHugTheSafeAreasRightEdgeBesideACutout() {
        Rect area = new Rect(118, 16, 2268, 1064);
        Rect topRight = OverlayPlacement.bounds(OverlayPlacement.Corner.TOP_RIGHT.spot(false), area, WIDTH, HEIGHT, false);
        assertEquals(new Rect(2268 - WIDTH, 16, 2268, 16 + HEIGHT), topRight);
        Rect bottomRight = OverlayPlacement.bounds(OverlayPlacement.Corner.BOTTOM_RIGHT.spot(false), area, WIDTH, HEIGHT, false);
        assertEquals(new Rect(2268 - WIDTH, 1064 - HEIGHT, 2268, 1064), bottomRight);
        Rect bottomLeft = OverlayPlacement.bounds(OverlayPlacement.Corner.BOTTOM_LEFT.spot(false), area, WIDTH, HEIGHT, false);
        assertEquals(new Rect(118, 1064 - HEIGHT, 118 + WIDTH, 1064), bottomLeft);
    }

    @Test
    public void theLayoutGearAndLeftStickMoveItToAFreeCorner() {
        List<Rect> controls = Arrays.asList(new Rect(15, 15, 111, 111), new Rect(100, 120, 460, 480));
        assertEquals(OverlayPlacement.Spot.TOP_END, OverlayPlacement.choose(AREA, WIDTH, HEIGHT, controls, false));
    }

    @Test
    public void theDefaultGamepadLeavesOnlyTheTopCentreFree() {
        // Gear, left stick and D-pad on the left; Y button and right stick on the right.
        List<Rect> gamepad = Arrays.asList(new Rect(15, 15, 111, 111), new Rect(100, 70, 470, 440),
                new Rect(70, 620, 500, 1050), new Rect(2070, 20, 2220, 160), new Rect(1960, 640, 2330, 1010),
                new Rect(520, 960, 690, 1060), new Rect(1220, 960, 1390, 1060), new Rect(1730, 960, 1900, 1060));
        OverlayPlacement.Spot spot = OverlayPlacement.choose(AREA, WIDTH, HEIGHT, gamepad, false);
        assertEquals(OverlayPlacement.Spot.TOP_CENTER, spot);
        Rect bounds = OverlayPlacement.bounds(spot, AREA, WIDTH, HEIGHT, false);
        for (Rect control : gamepad) assertFalse(control.toShortString(), Rect.intersects(bounds, control));
    }

    @Test
    public void whenEverySpotIsCoveredTheLeastCoveredWins() {
        List<Rect> controls = new ArrayList<>();
        for (OverlayPlacement.Spot spot : OverlayPlacement.Spot.values()) {
            Rect bounds = OverlayPlacement.bounds(spot, AREA, WIDTH, HEIGHT, false);
            int inset = spot == OverlayPlacement.Spot.BOTTOM_END ? 250 : 10;
            controls.add(new Rect(bounds.left + inset, bounds.top, bounds.right - inset, bounds.bottom));
        }
        assertEquals(OverlayPlacement.Spot.BOTTOM_END, OverlayPlacement.choose(AREA, WIDTH, HEIGHT, controls, false));
    }

    @Test
    public void rightToLeftStartsOnTheRight() {
        assertEquals(new Rect(1864, 16, 2384, 72),
                OverlayPlacement.bounds(OverlayPlacement.Spot.TOP_START, AREA, WIDTH, HEIGHT, true));
        List<Rect> gear = Collections.singletonList(new Rect(2280, 15, 2384, 111));
        assertEquals(OverlayPlacement.Spot.TOP_END, OverlayPlacement.choose(AREA, WIDTH, HEIGHT, gear, true));
    }

    @Test
    public void aFullPanelStreamKeepsTheOverlayOutOfTheCutoutInEitherLandscapeRotation() {
        Activity activity = Robolectric.buildActivity(Activity.class).setup().get();
        FrameLayout parent = new FrameLayout(activity);
        View overlay = new View(activity);
        parent.addView(overlay, new FrameLayout.LayoutParams(WIDTH, HEIGHT, Gravity.TOP | Gravity.START));
        activity.setContentView(parent, new android.view.ViewGroup.LayoutParams(2400, 1080));
        parent.layout(0, 0, 2400, 1080);
        // A camera cutout on the left after one rotation, on the right after the other.
        assertEquals(new Rect(118, 8, 2392, 1072), OverlayPlacement.safeArea(parent, Insets.of(110, 0, 0, 0), 8));
        assertEquals(new Rect(8, 8, 2282, 1072), OverlayPlacement.safeArea(parent, Insets.of(0, 0, 110, 0), 8));
        // A cutout-safe stream's padding already holds the cutout: it is not counted twice.
        parent.setPadding(110, 0, 0, 0);
        assertEquals(new Rect(118, 8, 2392, 1072), OverlayPlacement.safeArea(parent, Insets.of(110, 0, 0, 0), 8));
        // A half-opened fold pads the stream into one pane; the overlay stays in that pane.
        parent.setPadding(0, 0, 1220, 0);
        assertEquals(new Rect(8, 8, 1172, 1072), OverlayPlacement.safeArea(parent, Insets.NONE, 8));
    }

    @Test
    public void placingSetsGravityAndMarginsForTheChosenSpot() {
        Activity activity = Robolectric.buildActivity(Activity.class).setup().get();
        FrameLayout parent = new FrameLayout(activity);
        View overlay = new View(activity);
        View gear = new View(activity);
        parent.addView(overlay, new FrameLayout.LayoutParams(WIDTH, HEIGHT, Gravity.TOP | Gravity.START));
        FrameLayout.LayoutParams gearParams = new FrameLayout.LayoutParams(96, 96);
        gearParams.setMargins(15, 15, 0, 0);
        parent.addView(gear, gearParams);
        activity.setContentView(parent);
        parent.measure(View.MeasureSpec.makeMeasureSpec(2400, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(1080, View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 2400, 1080);
        OverlayPlacement.Spot spot = OverlayPlacement.place(overlay, parent,
                out -> OverlayPlacement.addVisible(gear, parent, out), 8);
        assertEquals(OverlayPlacement.Spot.TOP_END, spot);
        FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) overlay.getLayoutParams();
        assertEquals(Gravity.TOP | Gravity.END, params.gravity);
        assertEquals(8, params.topMargin);
        assertEquals(8, params.getMarginEnd());
        gear.setVisibility(View.INVISIBLE);
        assertEquals(OverlayPlacement.Spot.TOP_START, OverlayPlacement.place(overlay, parent,
                out -> OverlayPlacement.addVisible(gear, parent, out), 8));
        assertEquals(Gravity.TOP | Gravity.START, ((FrameLayout.LayoutParams) overlay.getLayoutParams()).gravity);
        // The chosen corner is taken at once, and its margins keep it inside the safe area.
        assertEquals(OverlayPlacement.Spot.BOTTOM_END, OverlayPlacement.place(overlay, parent, null, 8,
                OverlayPlacement.Corner.BOTTOM_RIGHT));
        FrameLayout.LayoutParams corner = (FrameLayout.LayoutParams) overlay.getLayoutParams();
        assertEquals(Gravity.BOTTOM | Gravity.END, corner.gravity);
        assertEquals(8, corner.bottomMargin);
        assertEquals(8, corner.getMarginEnd());
    }
}
