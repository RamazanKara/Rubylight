package com.limelight;

import android.app.Application;

import com.limelight.nvstream.AdaptiveBitrateController;
import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.io.IOException;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameHealthTest {
    @Test public void trialAndUndoUseTheLiveEncoderBudgetInsteadOfTheTransportPreference() throws Exception {
        FakeConnection connection = new FakeConnection(38988);
        Game game = game(connection);
        apply(game, false);
        assertEquals(1, connection.reads);
        assertEquals(1, connection.writes);
        assertEquals(31190, connection.lastWrite);
        assertEquals(38988, (int) ReflectionHelpers.getField(game, "healthOriginalBitrate"));
        assertEquals(31190, (int) ReflectionHelpers.getField(game, "healthTrialBitrate"));
        assertEquals(31190, (int) ReflectionHelpers.getField(game, "currentBitrate"));

        apply(game, true);
        assertEquals(2, connection.reads);
        assertEquals(2, connection.writes);
        assertEquals(38988, connection.lastWrite);
        assertEquals(38988, (int) ReflectionHelpers.getField(game, "currentBitrate"));
        assertNoTrial(game);
    }

    @Test public void undoPreservesAnInterveningHostChangeEvenWhenLocalDisplayStillShowsTheTrial() throws Exception {
        FakeConnection connection = new FakeConnection(38988);
        Game game = game(connection);
        apply(game, false);
        connection.getHostDetails().encoderBitrateKbps = 28000;

        apply(game, true);
        assertEquals(1, connection.writes);
        assertEquals(28000, (int) ReflectionHelpers.getField(game, "currentBitrate"));
        assertNoTrial(game);
    }

    @Test public void missingLiveBudgetNeverFallsBackToTheTransportPreference() throws Exception {
        FakeConnection connection = new FakeConnection(0);
        Game game = game(connection);
        apply(game, false);
        assertEquals(0, connection.writes);
        assertEquals(50000, (int) ReflectionHelpers.getField(game, "currentBitrate"));
        assertNoTrial(game);
    }

    @Test public void minimumEncoderBudgetDoesNotProduceAnIncreasingTrial() throws Exception {
        FakeConnection connection = new FakeConnection(400);
        Game game = game(connection);
        apply(game, false);
        assertEquals(0, connection.writes);
        assertEquals(400, (int) ReflectionHelpers.getField(game, "currentBitrate"));
        assertNoTrial(game);
    }

    @Test public void failedWriteDoesNotOfferUndoForATrialThatNeverApplied() throws Exception {
        FakeConnection connection = new FakeConnection(38988);
        connection.failWrite = true;
        Game game = game(connection);
        apply(game, false);
        assertEquals(1, connection.writes);
        assertEquals(50000, (int) ReflectionHelpers.getField(game, "currentBitrate"));
        assertNoTrial(game);
    }

    @Test public void stoppedSessionCannotApplyATrialAfterTheMetadataRead() throws Exception {
        FakeConnection connection = new FakeConnection(38988);
        Game game = game(connection);
        connection.afterRead = () -> ReflectionHelpers.setField(game, "connected", false);
        apply(game, false);
        assertEquals(0, connection.writes);
        assertNoTrial(game);
    }

    @Test public void automaticBitrateOrQualityOwnsTheSessionInsteadOfTheDiagnosticTrial() throws Exception {
        FakeConnection connection = new FakeConnection(38988);
        Game game = game(connection);
        ReflectionHelpers.setField(game, "adaptiveBitrate", new AdaptiveBitrateController(50000, 38988, 60, 0));
        apply(game, false);
        ReflectionHelpers.setField(game, "adaptiveBitrate", null);
        ReflectionHelpers.setField(game, "adaptiveQuality", new AdaptiveQualityController(
                AdaptiveQualityController.Mode.MOTION, 1920, 1080, 60, 720, 30, 2, 2, 0));
        apply(game, false);
        assertEquals(0, connection.reads);
        assertEquals(0, connection.writes);
        assertNoTrial(game);
    }

    private static Game game(FakeConnection connection) {
        Game game = Robolectric.buildActivity(Game.class).get();
        ReflectionHelpers.setField(game, "conn", connection);
        ReflectionHelpers.setField(game, "app", new NvApp("Game", 73, false));
        ReflectionHelpers.setField(game, "connected", true);
        ReflectionHelpers.setField(game, "foreground", true);
        ReflectionHelpers.setField(game, "currentBitrate", 50000);
        return game;
    }

    private static void apply(Game game, boolean undo) throws InterruptedException {
        ReflectionHelpers.callInstanceMethod(game.adaptation, "applyHealthBitrate",
                ReflectionHelpers.ClassParameter.from(boolean.class, undo));
        long deadline = System.nanoTime() + 2_000_000_000L;
        while ((boolean) ReflectionHelpers.getField(game, "hostActionInProgress") && System.nanoTime() < deadline) {
            ShadowLooper.idleMainLooper();
            Thread.sleep(5);
        }
        assertFalse(ReflectionHelpers.getField(game, "hostActionInProgress"));
    }

    private static void assertNoTrial(Game game) {
        assertEquals(0, (int) ReflectionHelpers.getField(game, "healthOriginalBitrate"));
        assertEquals(0, (int) ReflectionHelpers.getField(game, "healthTrialBitrate"));
    }

    private static class FakeConnection extends NvConnection {
        int reads, writes, lastWrite;
        boolean failWrite;
        Runnable afterRead;

        FakeConnection(int bitrate) {
            super(null, new ComputerDetails.AddressTuple("192.0.2.1", 47989), 47984, "test",
                    new StreamConfiguration.Builder().build(), null, null);
            ComputerDetails details = getHostDetails();
            details.caps = com.limelight.nvstream.http.TestHosts.rubylight();
            details.encoderBitrateSupported = true;
            details.permission = ComputerDetails.PERMISSION_VIEW;
            details.encoderBitrateKbps = bitrate;
        }

        @Override public ComputerDetails refreshHostDetails() {
            reads++;
            if (afterRead != null) afterRead.run();
            return getHostDetails();
        }

        @Override public int setBitrate(int bitrate) throws IOException {
            writes++;
            lastWrite = bitrate;
            if (failWrite) throw new IOException("temporary failure");
            getHostDetails().encoderBitrateKbps = bitrate;
            return bitrate;
        }
    }
}
