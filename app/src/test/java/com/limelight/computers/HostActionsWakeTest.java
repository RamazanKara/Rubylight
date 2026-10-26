package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.os.Looper;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.ToolbarIconTintTest;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowToast;

import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.TimeUnit;

/** Wake from the PC menus: the packets go out again while the PC stays away, and the wait ends loudly. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostActionsWakeTest {
    // The wake plan's resend interval and wait (WakeLaunch).
    private static final long RESEND_MS = 8_000;
    private static final long TIMEOUT_MS = 90_000;

    private ActivityController<ToolbarIconTintTest.Screen> controller;
    private HostActions actions;
    private long now;
    private final List<String> sent = new ArrayList<>();
    private final List<String> events = new ArrayList<>();
    private IOException sendError;
    private ComputerDetails pc;

    @Before
    public void setUp() {
        controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible();
        actions = new HostActions(controller.get(), new HostActions.Callbacks() {
            @Override public ComputerManagerService.ComputerManagerBinder binder() { return null; }
            @Override public boolean isForeground() { return true; }
            @Override public void onPaired(ComputerDetails computer) { }
            @Override public void onRemoved(ComputerDetails computer) { }
            @Override public void onRenamed(ComputerDetails computer) { }
            @Override public void onWakeSent(ComputerDetails computer) { events.add("sent " + computer.uuid); }
            @Override public void onWakeEnded(ComputerDetails computer) { events.add("ended " + computer.uuid); }
            @Override public void pausePolling() { }
            @Override public void resumePolling() { }
        });
        pc = pc("pc", "Office");
    }

    @After
    public void tearDown() {
        actions.destroy();
        controller.pause().stop().destroy();
    }

    private static ComputerDetails pc(String uuid, String name) {
        ComputerDetails pc = new ComputerDetails();
        pc.uuid = uuid;
        pc.name = name;
        pc.state = ComputerDetails.State.OFFLINE;
        pc.macAddress = "02:7c:16:b4:13:4f";
        return pc;
    }

    private void wake(ComputerDetails computer) {
        actions.startWake(computer, host -> {
            sent.add(host.uuid);
            if (sendError != null) throw sendError;
        }, Runnable::run, () -> now);
    }

    /** Lets time pass a second at a time, the waker's tick. */
    private void pass(long ms) {
        for (long step = 0; step < ms; step += 1_000) {
            now += 1_000;
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(1_000, TimeUnit.MILLISECONDS);
        }
    }

    private static String latestMessage() {
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
        assertNotNull(dialog);
        return ((TextView) dialog.findViewById(android.R.id.message)).getText().toString();
    }

    @Test
    public void thePcIsWokenUntilItAnswers() {
        wake(pc);
        assertEquals(Arrays.asList("pc"), sent);
        assertEquals(Arrays.asList("sent pc"), events);
        assertTrue(actions.isWaking());
        pass(RESEND_MS);
        assertEquals(Arrays.asList("pc", "pc"), sent);

        pc.state = ComputerDetails.State.ONLINE;
        pass(1_000);
        assertEquals(Arrays.asList("sent pc", "ended pc"), events);
        assertEquals(controller.get().getString(R.string.wol_pc_online), ShadowToast.getTextOfLatestToast());
        pass(TIMEOUT_MS);
        assertEquals(2, sent.size());
    }

    @Test
    public void aPcThatNeverAnswersEndsTheWaitWithTryAgain() {
        wake(pc);
        pass(TIMEOUT_MS + 1_000);
        assertEquals(Arrays.asList("sent pc", "ended pc"), events);
        assertEquals(controller.get().getString(R.string.console_wake_timeout, "Office"), latestMessage());
        int resent = sent.size();
        assertTrue(resent > 1);

        ((AlertDialog) ShadowDialog.getLatestDialog()).getButton(AlertDialog.BUTTON_POSITIVE).performClick();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertEquals("Try again wakes it again", resent + 1, sent.size());
    }

    @Test
    public void aWakeThatCannotSendSaysSo() {
        sendError = new IOException("no route");
        wake(pc);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertEquals(Arrays.asList("sent pc", "ended pc"), events);
        assertEquals(controller.get().getString(R.string.wol_fail), latestMessage());
        pass(TIMEOUT_MS);
        assertEquals(1, sent.size());
    }

    @Test
    public void wakingAnotherPcMovesTheWaitAndLeavingEndsIt() {
        wake(pc);
        wake(pc("den", "Den"));
        pass(RESEND_MS);
        assertEquals(Arrays.asList("pc", "den", "den"), sent);

        actions.destroy();
        pass(TIMEOUT_MS);
        assertEquals(3, sent.size());
        assertEquals(Arrays.asList("sent pc", "sent den"), events);
    }

    @Test
    public void anOnlinePcOrOneWithoutAMacIsNotWoken() {
        pc.state = ComputerDetails.State.ONLINE;
        wake(pc);
        assertEquals(controller.get().getString(R.string.wol_pc_online), ShadowToast.getTextOfLatestToast());

        ComputerDetails unknown = pc("den", "Den");
        unknown.macAddress = null;
        wake(unknown);
        assertEquals(controller.get().getString(R.string.wol_no_mac), latestMessage());
        assertTrue(sent.isEmpty());
        assertTrue(events.isEmpty());
    }
}
