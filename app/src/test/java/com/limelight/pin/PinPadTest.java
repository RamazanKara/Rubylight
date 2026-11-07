package com.limelight.pin;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.view.View;
import android.view.ViewGroup;

import com.limelight.R;
import com.limelight.ui.GlassPanelDrawable;
import com.limelight.ui.NightSkyView;
import com.limelight.ui.PinSlots;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/** The PIN stood bare on a grey page with a row-by-row pad: it is a glass panel under the sky, a grid of keys with one focus ring. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = android.app.Application.class, qualifiers = "w400dp-h800dp-port",
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class, instrumentedPackages = "com.limelight.nvstream.jni")
public class PinPadTest {
    private static PinLockActivity set() {
        return Robolectric.buildActivity(PinLockActivity.class,
                PinLockActivity.forSet(org.robolectric.RuntimeEnvironment.getApplication())).setup().get();
    }

    private static int[] order(PinLockActivity screen) {
        return new int[] {R.id.pin_key_ok, R.id.pin_key_0, R.id.pin_key_del};
    }

    @Test
    public void thePanelIsGlassUnderTheSky() {
        PinLockActivity screen = set();
        assertTrue(screen.findViewById(R.id.pin_panel).getBackground() instanceof GlassPanelDrawable);
        NightSkyView sky = screen.findViewById(R.id.pin_sky);
        assertNotNull(sky);
        assertTrue("the moon stands above a phone's sheet", sky.isMoonShown());
    }

    @Test
    @Config(qualifiers = "w800dp-h400dp-land")
    public void elsewhereItIsAPanelInTheMiddleWithNoMoonBehindIt() {
        PinLockActivity screen = set();
        assertFalse(((NightSkyView) screen.findViewById(R.id.pin_sky)).isMoonShown());
    }

    @Test
    public void everyKeyTakesFocusAndDeleteEndsTheLastRow() {
        PinLockActivity screen = set();
        for (int id : new int[] {R.id.pin_key_0, R.id.pin_key_1, R.id.pin_key_9, R.id.pin_key_del, R.id.pin_key_ok, R.id.pin_cancel}) {
            assertTrue(screen.findViewById(id).isFocusable());
        }
        ViewGroup row = (ViewGroup) screen.findViewById(R.id.pin_key_0).getParent();
        int[] ids = order(screen);
        for (int i = 0; i < ids.length; i++) assertSame(screen.findViewById(ids[i]), row.getChildAt(i));
    }

    @Test
    public void aNewPinGrowsItsSlotsAndRingsTheNextOne() {
        PinLockActivity screen = set();
        PinSlots slots = screen.findViewById(R.id.pin_dots);
        assertEquals(4, slots.getChildCount());
        assertTrue(slots.getChildAt(0).isActivated());
        for (int i = 0; i < 5; i++) screen.type(i);
        assertEquals("one more than typed", 6, slots.getChildCount());
        assertTrue(slots.getChildAt(5).isActivated());
        assertFalse(slots.getChildAt(0).isActivated());
        assertEquals("a PIN is never text in the view tree", "", slots.shown().toString());
        for (int i = 0; i < 8; i++) screen.type(i);
        assertEquals(8, slots.getChildCount());
        View last = slots.getChildAt(7);
        assertFalse("full: no next slot", last.isActivated());
    }

    @Test
    public void digitsAreDotsOnly() {
        PinLockActivity screen = set();
        screen.type(7);
        PinSlots slots = screen.findViewById(R.id.pin_dots);
        assertEquals("\u2022", ((android.widget.TextView) slots.getChildAt(0)).getText().toString());
    }
}
