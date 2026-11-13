package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.limelight.nativeapi.NativeGuests;

import org.junit.Test;

import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;

/** Which joins the device playing is asked about, and what it may change for each guest. */
public class StreamGuestsTest {
    private static NativeGuests guests(boolean manage, int keyboardMouse, List<NativeGuests.Guest> guests,
            List<NativeGuests.Request> waiting) {
        return new NativeGuests(manage, -1, "PC", NativeGuests.CONTROLLER, 4, 1, keyboardMouse, guests, waiting);
    }

    private static final NativeGuests.Request TABLET = new NativeGuests.Request(3, "Tablet", 40);
    private static final NativeGuests.Request TV = new NativeGuests.Request(4, "TV", 44);
    private static final List<NativeGuests.Guest> NONE = Collections.emptyList();

    @Test
    public void onlyTheManagerIsAskedAndEachRequestOnce() {
        NativeGuests read = guests(true, -1, NONE, Arrays.asList(TABLET, TV));
        assertEquals(Arrays.asList(TABLET, TV), StreamGuests.toAsk(read, new HashSet<>()));
        assertEquals(Collections.singletonList(TV), StreamGuests.toAsk(read, new HashSet<>(Collections.singleton(3))));
        assertTrue(StreamGuests.toAsk(guests(false, -1, NONE, Collections.singletonList(TABLET)), new HashSet<>()).isEmpty());
        assertTrue(StreamGuests.toAsk(null, new HashSet<>()).isEmpty());
    }

    @Test
    public void aPromptClosesOnceTheHostStopsWaiting() {
        NativeGuests waiting = guests(true, -1, NONE, Collections.singletonList(TABLET));
        assertFalse(StreamGuests.stale(waiting, 3));
        assertTrue(StreamGuests.stale(guests(true, -1, NONE, Collections.emptyList()), 3));
        assertTrue(StreamGuests.stale(guests(false, -1, NONE, Collections.singletonList(TABLET)), 3));
        assertFalse(StreamGuests.stale(waiting, -1));
    }

    @Test
    public void theManagerLooksOftenAndEveryoneElseRarely() {
        assertEquals(NativeGuests.POLL_MS, StreamGuests.nextPollMs(guests(true, -1, NONE, Collections.emptyList())));
        assertEquals(StreamGuests.IDLE_POLL_MS, StreamGuests.nextPollMs(guests(false, -1, NONE, Collections.emptyList())));
        assertEquals(StreamGuests.IDLE_POLL_MS, StreamGuests.nextPollMs(null));
    }

    @Test
    public void aGuestOffersTheOtherPermissionsTheKeyboardWhenItMayHaveItKickAndBlock() {
        NativeGuests.Guest pad = new NativeGuests.Guest(2, "Phone", NativeGuests.CONTROLLER, true);
        assertEquals(Arrays.asList(StreamGuests.Change.VIEW, StreamGuests.Change.KEYBOARD_MOUSE, StreamGuests.Change.KICK,
                StreamGuests.Change.BLOCK), StreamGuests.changes(guests(true, -1, Collections.singletonList(pad), Collections.emptyList()), pad));
        NativeGuests.Guest typing = new NativeGuests.Guest(2, "Phone", NativeGuests.KEYBOARD_MOUSE, true);
        assertTrue(StreamGuests.changes(guests(true, -1, Collections.singletonList(typing), Collections.emptyList()), typing)
                .contains(StreamGuests.Change.GIVE_KEYBOARD));
        assertTrue(StreamGuests.changes(guests(true, 2, Collections.singletonList(typing), Collections.emptyList()), typing)
                .contains(StreamGuests.Change.TAKE_KEYBOARD));
        NativeGuests.Guest away = new NativeGuests.Guest(2, "Phone", NativeGuests.KEYBOARD_MOUSE, false);
        List<StreamGuests.Change> awayChanges = StreamGuests.changes(guests(true, -1, Collections.singletonList(away), Collections.emptyList()), away);
        assertFalse(awayChanges.contains(StreamGuests.Change.GIVE_KEYBOARD));
        assertFalse(awayChanges.contains(StreamGuests.Change.TAKE_KEYBOARD));
    }
}
