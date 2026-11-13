package com.limelight.nativeapi;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.json.JSONObject;
import org.junit.Test;

/** GET /session/guests as the host answers it, and the endings that stop a guest's recovery. */
public class NativeGuestsTest {
    static final String FIXTURE = "{\"policy\":\"ask\",\"default_permission\":\"controller\",\"manage\":true,\"you\":null,"
            + "\"owner\":\"Living room PC\",\"max_guests\":4,\"version\":7,\"keyboard_mouse\":2,"
            + "\"guests\":[{\"id\":2,\"name\":\"Phone\",\"permission\":\"controller-keyboard-mouse\",\"streaming\":true,\"since\":10,\"expires\":null}],"
            + "\"waiting\":[{\"id\":3,\"name\":\"Tablet\",\"seconds_left\":40}]}";

    @Test
    public void readsTheHostsShape() throws Exception {
        NativeGuests read = NativeGuests.parse(new JSONObject(FIXTURE));
        assertTrue(read.manage);
        assertEquals(-1, read.you);
        assertEquals("Living room PC", read.owner);
        assertEquals(2, read.keyboardMouse);
        assertEquals(NativeGuests.KEYBOARD_MOUSE, read.guests.get(0).permission);
        assertTrue(read.guests.get(0).streaming);
        assertEquals("Tablet", read.waiting.get(0).name);
        assertEquals(40, read.waiting.get(0).secondsLeft);
    }

    @Test
    public void aSparseAnswerReadsAsNobody() throws Exception {
        NativeGuests read = NativeGuests.parse(new JSONObject("{\"policy\":\"allow\",\"owner\":null}"));
        assertFalse(read.manage);
        assertNull(read.owner);
        assertEquals(-1, read.keyboardMouse);
        assertTrue(read.guests.isEmpty() && read.waiting.isEmpty());
        assertEquals(NativeGuests.CONTROLLER, read.defaultPermission);
    }

    @Test
    public void theHostsEndingsStopRecoveryAndOthersDoNot() {
        assertTrue(NativeGuests.ended("The player removed you from the game"));
        assertTrue(NativeGuests.ended(" The game you joined ended"));
        assertTrue(NativeGuests.ended("Your time as a guest in this game ended"));
        assertTrue(NativeGuests.ended("The player does not let you join this game"));
        assertFalse(NativeGuests.ended("Encoder failed"));
        assertFalse(NativeGuests.ended(""));
        assertFalse(NativeGuests.ended(null));
    }
}
