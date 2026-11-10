package com.limelight.nvstream.http;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import android.app.Application;

import com.limelight.nativeapi.NativeApiFixtures;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.lang.reflect.Field;
import java.lang.reflect.Modifier;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * The screens redraw a PC only when {@link ComputerDetails#fields} says a poll brought news, so every field has
 * to be in it: one left out could change without the screen ever showing it.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ComputerDetailsFieldsTest {
    private static List<Field> instanceFields() {
        List<Field> fields = new ArrayList<>();
        for (Field field : ComputerDetails.class.getDeclaredFields()) {
            // Coverage tools add their own; those are synthetic.
            if (!Modifier.isStatic(field.getModifiers()) && !field.isSynthetic()) {
                field.setAccessible(true);
                fields.add(field);
            }
        }
        return fields;
    }

    /** A value of the field's type that differs from {@code now}. */
    private static Object other(Field field, Object now) {
        Class<?> type = field.getType();
        if (type == String.class) return "other".equals(now) ? "another" : "other";
        if (type == int.class) return (Integer) now + 1;
        if (type == long.class) return (Long) now + 1;
        if (type == boolean.class) return !(Boolean) now;
        if (type == ComputerDetails.AddressTuple.class) return new ComputerDetails.AddressTuple("10.0.0.9", 47989);
        if (type == ComputerDetails.State.class) {
            return now == ComputerDetails.State.ONLINE ? ComputerDetails.State.OFFLINE : ComputerDetails.State.ONLINE;
        }
        if (type == PairingManager.PairState.class) {
            return now == PairingManager.PairState.PAIRED ? PairingManager.PairState.NOT_PAIRED
                    : PairingManager.PairState.PAIRED;
        }
        if (type == X509Certificate.class) return NativeApiFixtures.certificate();
        if (type == HostCapabilities.class) return TestHosts.rubylight();
        if (type == ArrayList.class) {
            ArrayList<String> list = new ArrayList<>();
            list.add("a command");
            return list;
        }
        fail("Teach this test a second value for " + field.getName() + " (" + type.getName() + ")");
        return null;
    }

    @Test
    public void aChangeToAnyFieldIsNews() throws Exception {
        List<Field> fields = instanceFields();
        assertEquals("every field, once", fields.size(), new ComputerDetails().fields().length);
        for (Field field : fields) {
            ComputerDetails before = new ComputerDetails();
            ComputerDetails after = new ComputerDetails();
            assertTrue(Arrays.deepEquals(before.fields(), after.fields()));
            Object changed = other(field, field.get(after));
            assertFalse(field.getName() + " needs a second value", java.util.Objects.equals(changed, field.get(before)));
            field.set(after, changed);
            assertFalse(field.getName() + " changed and fields() did not say so",
                    Arrays.deepEquals(before.fields(), after.fields()));
        }
    }

    @Test
    public void aPollsCopyOfAnUnchangedPcIsNoNews() throws Exception {
        ComputerDetails polled = new ComputerDetails();
        // Every field set to something, so a field the comparison takes by identity would show.
        for (Field field : instanceFields()) {
            field.set(polled, other(field, field.get(polled)));
        }
        polled.serverCommands = new ArrayList<>(Arrays.asList("lock", "sleep"));
        ComputerDetails copy = new ComputerDetails(polled);
        copy.serverCommands = new ArrayList<>(polled.serverCommands);
        copy.localAddress = new ComputerDetails.AddressTuple("10.0.0.9", 47989);
        copy.name = new String(polled.name);
        assertTrue(Arrays.deepEquals(polled.fields(), copy.fields()));
    }

    @Test
    public void theValuesAreTakenWhenAskedSoALaterChangeInPlaceIsNews() {
        ComputerDetails live = new ComputerDetails();
        live.serverCommands.add("lock");
        Object[] before = live.fields();
        live.serverCommands.add("sleep");
        assertFalse(Arrays.deepEquals(before, live.fields()));
        Object[] between = live.fields();
        live.state = ComputerDetails.State.ONLINE;
        assertFalse(Arrays.deepEquals(between, live.fields()));
    }
}
