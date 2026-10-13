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
    @Test public void boundsZoomPanResizeAndResetNeverRevealEmptyArea() {
        DesktopViewport view = new DesktopViewport(); view.resize(1000, 500);
        view.zoom(100, 500, 250); assertEquals(4, view.scale(), 0);
        view.pan(100000, -100000); assertEquals(0, view.x(), 0); assertEquals(-1500, view.y(), 0);
        assertEquals(0, view.inputX(-100), 0); assertEquals(500, view.inputY(10000), 0);
        view.resize(500, 1000); assertEquals(-3000, view.y(), 0);
        view.zoom(Float.NaN, 0, 0); view.pan(Float.POSITIVE_INFINITY, 0);
        assertEquals(4, view.scale(), 0); assertEquals(0, view.x(), 0);
        view.reset(); assertEquals(1, view.scale(), 0); assertEquals(0, view.y(), 0);
        view.zoom(.1f, 300, 100); assertEquals(1, view.scale(), 0);
    }
}
