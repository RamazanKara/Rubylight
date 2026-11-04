package com.limelight.binding.input;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;
import android.view.KeyEvent;

import com.limelight.ui.RustStreamMenuModel;
import com.limelight.ui.StreamMenuModel;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/** Android's stream hotkeys: the shared table's defaults, a changed chord, Off and Reset, and the old chords' one notice. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class StreamHotkeysTest {
    private static final int CTRL_ALT_SHIFT = 0x07;
    private SharedPreferences prefs;

    @Before
    public void model() {
        StreamMenuModel.use(new RustStreamMenuModel());
        prefs = PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
        prefs.edit().clear().commit();
    }

    @After
    public void forgetTheModel() {
        StreamMenuModel.use(null);
    }

    private StreamHotkeys.Table table() throws StreamMenuModel.Refused {
        return new StreamHotkeys.Table(StreamHotkeys.load(prefs));
    }

    private static String id(StreamHotkeys.Match match) {
        return match == null ? null : match.hotkey.id;
    }

    @Test
    public void theDefaultsAreMoonlightsAndTheOldMenuAndCursorChordsStillWork() throws Exception {
        StreamHotkeys.Table table = table();
        assertEquals(StreamHotkeys.STREAM_MENU, id(table.at(CTRL_ALT_SHIFT, 0x09)));
        assertEquals(StreamHotkeys.QUIT, id(table.at(CTRL_ALT_SHIFT, 'Q')));
        assertEquals(StreamHotkeys.CAPTURE, id(table.at(CTRL_ALT_SHIFT, 'Z')));
        assertEquals(StreamHotkeys.OVERLAY, id(table.at(CTRL_ALT_SHIFT, 'S')));
        assertEquals(StreamHotkeys.PC_KEYBOARD, id(table.at(CTRL_ALT_SHIFT, 'K')));
        assertEquals(StreamHotkeys.CURSOR, id(table.at(CTRL_ALT_SHIFT, 'P')));
        StreamHotkeys.Match menu = table.at(CTRL_ALT_SHIFT, 'M');
        assertEquals(StreamHotkeys.STREAM_MENU, id(menu));
        assertEquals("Ctrl + Alt + Shift + M", menu.old.label);
        assertEquals(StreamHotkeys.CURSOR, id(table.at(CTRL_ALT_SHIFT, 'C')));
        // The exact modifiers: Ctrl+Alt+Q or Ctrl+Alt+Shift+Win+Q goes to the PC.
        assertNull(table.at(0x06, 'Q'));
        assertNull(table.at(CTRL_ALT_SHIFT | 0x08, 'Q'));
    }

    @Test
    public void aChangedChordFiresAtOnceAndTheDefaultAndOldChordGoToThePc() throws Exception {
        StreamHotkeys.record(prefs, StreamHotkeys.STREAM_MENU, new String[] {"F2", "CTRL"});
        assertEquals("CTRL+F2", prefs.getString(StreamHotkeys.PREFIX + StreamHotkeys.STREAM_MENU, null));
        StreamHotkeys.Table table = table();
        assertEquals(StreamHotkeys.STREAM_MENU, id(table.at(0x02, 0x71)));
        assertNull(table.at(CTRL_ALT_SHIFT, 0x09));
        assertNull(table.at(CTRL_ALT_SHIFT, 'M'));

        StreamHotkeys.turnOff(prefs, StreamHotkeys.QUIT);
        assertNull(table().at(CTRL_ALT_SHIFT, 'Q'));
        StreamHotkeys.reset(prefs, StreamHotkeys.QUIT);
        assertEquals(StreamHotkeys.QUIT, id(table().at(CTRL_ALT_SHIFT, 'Q')));
    }

    @Test
    public void aChordAnotherActionHasIsRefusedNamingIt() {
        try {
            StreamHotkeys.record(prefs, StreamHotkeys.CURSOR, new String[] {"CTRL", "ALT", "SHIFT", "Q"});
            fail("taken");
        } catch (StreamMenuModel.Refused e) {
            assertEquals("taken", e.code);
            assertEquals(StreamHotkeys.QUIT, e.detail);
        }
        assertFalse(prefs.contains(StreamHotkeys.PREFIX + StreamHotkeys.CURSOR));
    }

    @Test
    public void anOldChordSaysOnceWhereItMoved() throws Exception {
        StreamHotkeys.Match menu = table().at(CTRL_ALT_SHIFT, 'M');
        assertTrue(menu.firstOldUse(prefs));
        assertFalse(table().at(CTRL_ALT_SHIFT, 'M').firstOldUse(prefs));
        assertFalse(table().at(CTRL_ALT_SHIFT, 0x09).firstOldUse(prefs));
        // The notice's flag is no binding.
        assertTrue(StreamHotkeys.bindings(prefs).isEmpty());
    }

    @Test
    public void bothSidesOfAModifierRecordAsOneName() {
        assertEquals("CTRL", StreamHotkeys.keyName(KeyEvent.KEYCODE_CTRL_RIGHT));
        assertEquals("SHIFT", StreamHotkeys.keyName(KeyEvent.KEYCODE_SHIFT_LEFT));
        assertEquals("ALT", StreamHotkeys.keyName(KeyEvent.KEYCODE_ALT_RIGHT));
        assertEquals("TAB", StreamHotkeys.keyName(KeyEvent.KEYCODE_TAB));
        assertEquals("F2", StreamHotkeys.keyName(KeyEvent.KEYCODE_F2));
    }
}
