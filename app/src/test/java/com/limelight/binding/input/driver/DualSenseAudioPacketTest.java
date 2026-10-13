package com.limelight.binding.input.driver;

import org.junit.Test;
import java.util.Arrays;
import static org.junit.Assert.*;

public class DualSenseAudioPacketTest {
    @Test public void microphonePacketsRoundTripAndRejectMalformedInput() {
        byte[] samples = new byte[784];
        Arrays.fill(samples, (byte)42);
        byte[] packet = DualSenseAudioPacket.pcm(15, -1, samples, samples.length);
        assertTrue(DualSenseAudioPacket.validPcm(packet));
        assertEquals(15, DualSenseAudioPacket.id(packet));
        assertEquals(-1, DualSenseAudioPacket.sequence(packet));
        assertArrayEquals(samples, Arrays.copyOfRange(packet, 12, packet.length));
        assertFalse(DualSenseAudioPacket.validPcm(Arrays.copyOf(packet, packet.length - 1)));
        packet[10] = 1;
        assertFalse(DualSenseAudioPacket.validPcm(packet));
        assertNull(DualSenseAudioPacket.pcm(16, 0, samples, 4));
        assertNull(DualSenseAudioPacket.pcm(0, 0, samples, 3));
        assertFalse(DualSenseAudioPacket.validControl(new byte[] {1, 2}));
    }

    @Test public void speakerAndHapticsCanShareAUsbTransferWithoutOverwritingEachOther() {
        DualSenseHapticsQueue haptics = new DualSenseHapticsQueue();
        DualSenseHapticsQueue speaker = new DualSenseHapticsQueue();
        haptics.offer(0, new byte[] {1, 2, 3, 4}, 100);
        speaker.offer(0, new byte[] {5, 6, 7, 8}, 100);
        byte[] usb = new byte[DualSenseHapticsQueue.USB_BYTES];
        haptics.render(usb, 100);
        speaker.render(usb, 100, 0, false);
        assertArrayEquals(new byte[] {5, 6, 7, 8, 1, 2, 3, 4}, Arrays.copyOf(usb, 8));
    }

    @Test public void audioControlsPreserveTriggersAndLocalMuteWins() {
        DualSenseReport.Output output = new DualSenseReport.Output(true);
        output.audio(false, true);
        byte[] control = {1, 2, 0, 0, (byte)0xf0, 3, 80, 61, 64, 0x30, 0, 0};
        byte[] report = output.audioControl(control);
        assertEquals(0x10, report[10] & 0x10);
        assertEquals(1, report[9]);
        output.waveform(true);
        report = output.audio(true, false);
        assertEquals(0, report[8] & 0x30);
        assertEquals(0, report[1] & 3);
        assertEquals(0, report[10] & 0x10);
    }
}
