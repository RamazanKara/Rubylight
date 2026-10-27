package com.limelight.binding.input;

import android.app.Application;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class TouchLayoutTest {
    private static TouchLayout.Button button(String id, int... keys) {
        return new TouchLayout.Button(id, id, keys, 0, false, .1f, .2f, .15f, .2f);
    }
    @Test public void portableKeyboardAndMouseLayoutsRoundTripAndStayPerGame() {
        TouchLayout layout = new TouchLayout(Arrays.asList(button("jump", 62),
                new TouchLayout.Button("fire", "Fire", new int[0], 1, true, .5f, .5f, .2f, .2f)));
        TouchLayout restored = TouchLayout.parse(layout.encode());
        assertEquals(2, restored.buttons.size()); assertEquals(1, restored.buttons.get(1).mouseButton);
        assertTrue(restored.buttons.get(1).toggle); assertEquals(.1f, restored.buttons.get(0).x, .0001f);
        String gameA = TouchLayout.profileKey("host", "gameA", 1), gameB = TouchLayout.profileKey("host", "gameB", 1);
        layout.save(RuntimeEnvironment.getApplication(), gameA);
        assertEquals(2, TouchLayout.load(RuntimeEnvironment.getApplication(), gameA).buttons.size());
        assertTrue(TouchLayout.load(RuntimeEnvironment.getApplication(), gameB).buttons.isEmpty());
        assertNotEquals(TouchLayout.profileKey("a", "bc", 1), TouchLayout.profileKey("ab", "c", 1));
    }
    @Test public void rejectsCorruptOversizedAndOffscreenControlsWithoutMutatingSavedLayout() {
        assertThrows(IllegalArgumentException.class, () -> new TouchLayout(Arrays.asList(button("same", 29), button("same", 30))));
        assertThrows(IllegalArgumentException.class, () -> new TouchLayout.Button("x", "X", new int[]{29}, 0, false, .9f, 0, .2f, .2f));
        assertThrows(IllegalArgumentException.class, () -> new TouchLayout.Button("x", "X", new int[]{29}, 0, false, Float.NaN, 0, .2f, .2f));
        assertThrows(IllegalArgumentException.class, () -> TouchLayout.parse("{\"version\":3,\"buttons\":[]}"));
        assertThrows(IllegalArgumentException.class, () -> TouchLayout.parse("{\"version\":1.5,\"buttons\":[]}"));
        assertThrows(IllegalArgumentException.class, () -> TouchLayout.parse("{\"version\":\"1\",\"buttons\":[]}"));
        assertThrows(IOException.class, () -> TouchLayout.read(new ByteArrayInputStream(new byte[TouchLayout.MAX_BYTES + 1])));
        List<TouchLayout.Button> tooMany = new ArrayList<>();
        for (int i = 0; i < 33; i++) tooMany.add(button("button" + i, 29));
        assertThrows(IllegalArgumentException.class, () -> new TouchLayout(tooMany));
        TouchLayout.Button moved = button("moving", 29).move(-1, 100);
        assertEquals(0, moved.x, 0); assertEquals(.8f, moved.y, .00001f);
    }
    @Test public void sharedChordModifiersAndCancelDoNotLeaveKeysHeld() {
        List<String> events = new ArrayList<>();
        HeldInputState held = new HeldInputState(new HeldInputState.Sink() {
            public void key(int key, boolean down) { events.add(key + ":" + down); }
            public void mouse(int button, boolean down) { events.add("m" + button + ":" + down); }
        });
        held.press(button("copy", 113, 31)); held.press(button("paste", 113, 50));
        held.press(button("paste", 113, 50)); held.release("copy");
        assertEquals(Arrays.asList("113:true", "31:true", "50:true", "31:false"), events);
        held.releaseAll(); held.releaseAll(); held.release("missing");
        assertEquals(Arrays.asList("113:true", "31:true", "50:true", "31:false", "50:false", "113:false"), events);
    }
    @Test public void mouseSharedTargetsAndLayoutReplacementReleaseExactlyOnce() {
        List<String> events = new ArrayList<>();
        HeldInputState held = new HeldInputState(new HeldInputState.Sink() {
            public void key(int key, boolean down) { fail(); }
            public void mouse(int button, boolean down) { events.add(button + ":" + down); }
        });
        held.press(new TouchLayout.Button("one", "One", new int[0], 1, true, 0, 0, .2f, .2f));
        held.press(new TouchLayout.Button("two", "Two", new int[0], 1, false, .5f, .5f, .2f, .2f));
        held.release("one"); assertEquals(Collections.singletonList("1:true"), events);
        held.releaseAll(); assertEquals(Arrays.asList("1:true", "1:false"), events);
    }
}
