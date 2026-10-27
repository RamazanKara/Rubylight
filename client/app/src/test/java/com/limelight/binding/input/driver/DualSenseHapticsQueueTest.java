package com.limelight.binding.input.driver;

import org.junit.Test;
import java.util.Arrays;
import static org.junit.Assert.*;

public class DualSenseHapticsQueueTest {
    private static final long NOW = 1_000_000_000L;

    private static byte[] pcm(int frames, int value) {
        byte[] data = new byte[frames * 4];
        Arrays.fill(data, (byte)value);
        return data;
    }

    @Test
    public void preservesSignedActuatorChannelsAndNeverWritesSpeakerChannels() {
        DualSenseHapticsQueue queue = new DualSenseHapticsQueue();
        byte[] input = {(byte)0xff, 0x7f, 0, (byte)0x80, 0x34, 0x12, (byte)0xcc, (byte)0xed};
        queue.offer(10, input, NOW);
        input[0] = 0; // Callback data is borrowed; queue must own its copy.
        byte[] usb = new byte[DualSenseHapticsQueue.USB_BYTES];
        assertTrue(queue.render(usb, NOW));
        assertArrayEquals(new byte[] {0, 0, 0, 0, (byte)0xff, 0x7f, 0, (byte)0x80,
                0, 0, 0, 0, 0x34, 0x12, (byte)0xcc, (byte)0xed}, Arrays.copyOf(usb, 16));
        assertArrayEquals(new byte[usb.length - 16], Arrays.copyOfRange(usb, 16, usb.length));
    }

    @Test
    public void carriesFiveMillisecondBlocksAcrossFourMillisecondUsbSubmissions() {
        DualSenseHapticsQueue queue = new DualSenseHapticsQueue();
        queue.offer(0, pcm(240, 1), NOW);
        queue.offer(1, pcm(240, 2), NOW);
        byte[] usb = new byte[DualSenseHapticsQueue.USB_BYTES];
        queue.render(usb, NOW);
        queue.render(usb, NOW + 4_000_000L);
        for (int frame = 0; frame < 192; frame++) assertEquals(frame < 48 ? 1 : 2, usb[frame * 8 + 4]);
    }

    @Test
    public void rejectsLateAndDuplicateBlocksAcrossSequenceWrapAndDropsDataBeforeLoss() {
        DualSenseHapticsQueue queue = new DualSenseHapticsQueue();
        queue.offer(-1, pcm(1, 1), NOW);
        queue.offer(0, pcm(1, 2), NOW);
        queue.offer(-1, pcm(1, 9), NOW);
        queue.offer(0, pcm(1, 9), NOW);
        byte[] usb = new byte[DualSenseHapticsQueue.USB_BYTES];
        queue.render(usb, NOW);
        assertEquals(1, usb[4]);
        assertEquals(2, usb[12]);
        assertEquals(0, usb[20]);
        queue.offer(1, pcm(240, 3), NOW);
        queue.offer(3, pcm(1, 4), NOW);
        queue.render(usb, NOW);
        assertEquals(4, usb[4]);
        assertEquals(0, usb[12]);
    }

    @Test
    public void boundsBacklogAndDropsStaleSamplesAfterASchedulerStall() {
        DualSenseHapticsQueue queue = new DualSenseHapticsQueue();
        for (int i = 0; i < 5; i++) queue.offer(i, pcm(240, i + 1), NOW);
        byte[] usb = new byte[DualSenseHapticsQueue.USB_BYTES];
        queue.render(usb, NOW);
        assertEquals(5, usb[4]);
        queue.render(usb, NOW + DualSenseHapticsQueue.STALE_NS);
        assertArrayEquals(new byte[usb.length], usb);
    }

    @Test
    public void silentHostCannotKeepRumbleDisabledAndOldDataCannotRestartIt() {
        DualSenseHapticsQueue queue = new DualSenseHapticsQueue();
        queue.offer(0, pcm(240, 0), NOW);
        assertFalse(queue.active(NOW));
        queue.offer(1, pcm(240, 1), NOW);
        queue.offer(2, pcm(240, 0), NOW + DualSenseHapticsQueue.IDLE_NS);
        assertFalse(queue.active(NOW + DualSenseHapticsQueue.IDLE_NS));
        queue.offer(1, pcm(240, 1), NOW + DualSenseHapticsQueue.IDLE_NS);
        assertFalse(queue.active(NOW + DualSenseHapticsQueue.IDLE_NS));
        queue.clear();
        queue.offer(0, pcm(1, 2), NOW + DualSenseHapticsQueue.IDLE_NS);
        assertTrue(queue.active(NOW + DualSenseHapticsQueue.IDLE_NS));
    }

    @Test
    public void rejectsMalformedSamplesAndKeepsControllersIndependent() {
        DualSenseHapticsQueue first = new DualSenseHapticsQueue();
        DualSenseHapticsQueue second = new DualSenseHapticsQueue();
        for (byte[] invalid : new byte[][] {null, new byte[0], new byte[3], pcm(241, 1)}) {
            first.offer(10, invalid, NOW);
        }
        first.offer(0, pcm(1, 1), NOW);
        second.offer(0, pcm(1, 2), NOW);
        first.clear();
        byte[] usb = new byte[DualSenseHapticsQueue.USB_BYTES];
        first.render(usb, NOW);
        assertArrayEquals(new byte[usb.length], usb);
        second.render(usb, NOW);
        assertEquals(2, usb[4]);
    }
}
