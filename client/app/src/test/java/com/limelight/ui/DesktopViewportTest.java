package com.limelight.ui;

import org.junit.Test;
import static org.junit.Assert.*;

public class DesktopViewportTest {
    @Test public void zoomKeepsTheFocusedHostPointAndInverseMapping() {
        DesktopViewport view = new DesktopViewport(); view.resize(1200, 800);
        view.zoom(2, 600, 400);
        assertEquals(600, view.inputX(600), .001f); assertEquals(400, view.inputY(400), .001f);
        assertEquals(400, view.inputX(200), .001f); assertEquals(300, view.inputY(200), .001f);
        view.pan(100, -100);
        assertEquals(350, view.inputX(200), .001f); assertEquals(350, view.inputY(200), .001f);
    }
    @Test public void boundsZoomOutClampingResizeAndResetKeepMinimumVisibleMargin() {
        DesktopViewport view = new DesktopViewport(); view.resize(1000, 500);
        assertFalse(view.isTransformed());
        view.zoom(100, 500, 250); assertEquals(4, view.scale(), 0);
        assertTrue(view.isTransformed());
        view.pan(100000, -100000); assertEquals(800, view.x(), 0); assertEquals(-1900, view.y(), 0);
        assertEquals(0, view.inputX(-100), 0); assertEquals(500, view.inputY(10000), 0);
        view.resize(500, 1000); assertEquals(400, view.x(), 0); assertEquals(-3800, view.y(), 0);
        view.zoom(Float.NaN, 0, 0); view.pan(Float.POSITIVE_INFINITY, 0);
        assertEquals(4, view.scale(), 0); assertEquals(400, view.x(), 0);
        view.reset(); assertEquals(1, view.scale(), 0); assertEquals(0, view.y(), 0);
        assertFalse(view.isTransformed());
        view.zoom(.1f, 300, 100); assertEquals(DesktopViewport.MIN_SCALE, view.scale(), 0);
        assertTrue(view.isTransformed());
    }

    @Test public void unrestrictedPanAtOneXPullsEdgesAndCornersInwardWhileKeepingMargin() {
        DesktopViewport view = new DesktopViewport(); view.resize(2400, 1080);
        view.pan(-1200, 540);
        assertTrue(view.isTransformed());
        assertEquals(1, view.scale(), 0);
        assertEquals(2400, view.inputX(1200), .001f);
        assertEquals(0, view.inputY(540), .001f);

        view.reset();
        view.pan(1200, -540);
        assertEquals(0, view.inputX(1200), .001f);
        assertEquals(1080, view.inputY(540), .001f);

        view.pan(100000, -100000);
        assertEquals(1920, view.x(), 0);
        assertEquals(-864, view.y(), 0);
    }

    @Test public void containerBoundsAllowFitVideoToExpandAndPanAcrossBlackBorders() {
        DesktopViewport view = new DesktopViewport();
        view.setBounds(1920, 1080, 2400, 1080, 240, 0);
        view.zoom(1.25f, 960, 540);
        assertEquals(1.25f, view.scale(), .001f);
        assertEquals(-240f, view.x(), .01f);
        assertEquals(-135f, view.y(), .01f);

        view.reset();
        view.pan(-240, 0);
        assertEquals(-240f, view.x(), 0);
        view.pan(480, 0);
        assertEquals(240f, view.x(), 0);
    }
}
