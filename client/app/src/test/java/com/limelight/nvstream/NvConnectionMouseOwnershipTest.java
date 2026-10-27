package com.limelight.nvstream;

import com.limelight.binding.input.MouseButtonOwnership;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Test;

import java.lang.reflect.Field;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.Assert.*;

public class NvConnectionMouseOwnershipTest {
    @Test
    public void forbiddenReleasesClearLocalOwnershipWithoutReachingTheNativeBridge() throws Exception {
        for (int source = MouseButtonOwnership.ORDINARY; source <= MouseButtonOwnership.DESKTOP_CONTROLS; source++) {
            Connection connection = new Connection();
            MouseButtonOwnership ownership = field(connection, "mouseButtonOwnership");
            assertEquals(1, ownership.update(1, true, source));
            connection.inputAllowed = false;

            send(connection, source, false);

            // No native library is loaded: any attempted forbidden send also fails this test.
            connection.inputAllowed = true;
            assertEquals("Release left stale owner " + source, 1, ownership.update(1, true, source));
        }
    }

    @Test
    public void forbiddenCleanupPreservesAnotherSurfacesHold() throws Exception {
        for (int source = MouseButtonOwnership.ORDINARY; source <= MouseButtonOwnership.DESKTOP_CONTROLS; source++) {
            Connection connection = new Connection();
            MouseButtonOwnership ownership = field(connection, "mouseButtonOwnership");
            int other = (source + 1) % 3;
            ownership.update(1, true, source);
            ownership.update(1, true, other);
            connection.inputAllowed = false;

            send(connection, source, false);
            send(connection, source, true);

            assertEquals("Cleanup or denied press changed another owner", -1, ownership.update(1, false, other));
        }
    }

    @Test
    public void permissionLostWhileWaitingForBatchDoesNotReserveAButton() throws Exception {
        for (int source = MouseButtonOwnership.ORDINARY; source <= MouseButtonOwnership.DESKTOP_CONTROLS; source++) {
            Connection connection = new Connection();
            MouseButtonOwnership ownership = field(connection, "mouseButtonOwnership");
            Object batcher = field(connection, "inputBatcher");
            AtomicReference<Throwable> failure = new AtomicReference<>();
            final int owner = source;
            Thread input = new Thread(() -> {
                try { send(connection, owner, true); }
                catch (Throwable error) { failure.set(error); }
            });
            Thread.State waitingState;
            synchronized (batcher) {
                input.start();
                long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2);
                while (input.isAlive() && input.getState() != Thread.State.BLOCKED && System.nanoTime() < deadline) {
                    Thread.sleep(1);
                }
                waitingState = input.getState();
                connection.inputAllowed = false;
            }
            input.join(2000);

            assertEquals(Thread.State.BLOCKED, waitingState);
            assertFalse("Input thread did not finish", input.isAlive());
            assertNull("Forbidden input reached JNI", failure.get());
            assertEquals("A cancelled press retained ownership", 1, ownership.update(1, true, source));
        }
    }

    private static void send(Connection connection, int source, boolean down) {
        if (source == MouseButtonOwnership.DESKTOP_CONTROLS) connection.sendDesktopMouseButton((byte) 1, down);
        else if (source == MouseButtonOwnership.GAME_CONTROLS) connection.sendOverlayMouseButton((byte) 1, down);
        else if (down) connection.sendMouseButtonDown((byte) 1);
        else connection.sendMouseButtonUp((byte) 1);
    }

    @SuppressWarnings("unchecked")
    private static <T> T field(Connection connection, String name) throws Exception {
        Field field = NvConnection.class.getDeclaredField(name);
        field.setAccessible(true);
        return (T) field.get(connection);
    }

    private static final class Connection extends NvConnection {
        volatile boolean inputAllowed = true;

        Connection() {
            super(null, new ComputerDetails.AddressTuple("127.0.0.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
        }

        @Override public boolean canSendInput() { return inputAllowed; }
    }
}
