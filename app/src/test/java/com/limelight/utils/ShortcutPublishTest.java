package com.limelight.utils;

import android.app.Activity;
import android.app.Application;
import android.content.Intent;
import android.content.pm.ShortcutInfo;
import android.content.pm.ShortcutManager;

import com.limelight.nvstream.http.ComputerDetails;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowShortcutManager;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

import static org.junit.Assert.*;

/**
 * Every poll of an online PC asks for its shortcut. The shortcut is made of the PC's UUID and name, so only a poll
 * that changed one of them may reach the system's shortcut service, which answers over a binder call each time.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = ShortcutPublishTest.CountingShortcutManager.class)
public class ShortcutPublishTest {
    /** Counts the calls that reach the shortcut service. */
    @Implements(ShortcutManager.class)
    public static class CountingShortcutManager extends ShadowShortcutManager {
        static int calls;

        @Implementation
        protected List<ShortcutInfo> getDynamicShortcuts() {
            calls++;
            return super.getDynamicShortcuts();
        }

        @Implementation
        protected List<ShortcutInfo> getPinnedShortcuts() {
            calls++;
            return super.getPinnedShortcuts();
        }

        @Implementation
        protected boolean addDynamicShortcuts(List<ShortcutInfo> shortcuts) {
            calls++;
            return super.addDynamicShortcuts(shortcuts);
        }

        @Implementation
        protected boolean updateShortcuts(List<ShortcutInfo> shortcuts) {
            calls++;
            return super.updateShortcuts(shortcuts);
        }
    }

    private Activity activity;
    private ShortcutManager manager;
    private ShortcutHelper helper;

    @Before
    public void open() {
        activity = Robolectric.buildActivity(Activity.class).create().get();
        manager = activity.getSystemService(ShortcutManager.class);
        helper = new ShortcutHelper(activity);
        CountingShortcutManager.calls = 0;
    }

    @After
    public void close() {
        activity.finish();
    }

    private static ComputerDetails pc(String uuid, String name) {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = uuid;
        computer.name = name;
        return computer;
    }

    /** What the shortcut service was asked since the last look. */
    private static int asked() {
        int calls = CountingShortcutManager.calls;
        CountingShortcutManager.calls = 0;
        return calls;
    }

    private String label(String uuid) {
        CharSequence label = null;
        for (ShortcutInfo info : manager.getDynamicShortcuts()) {
            if (info.getId().equals(uuid)) {
                label = info.getShortLabel();
            }
        }
        asked();
        return label == null ? null : label.toString();
    }

    @Test
    public void aPollThatChangedNeitherUuidNorNameAsksTheShortcutServiceNothing() {
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        assertTrue("the first poll publishes", asked() > 0);
        assertEquals("Tower", label("tower"));

        // The polls report copies, and other fields change freely.
        ComputerDetails again = pc("tower", "Tower");
        again.state = ComputerDetails.State.ONLINE;
        again.runningGameId = 7;
        helper.createAppViewShortcutForOnlineHost(again);
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        assertEquals(0, asked());
    }

    @Test
    public void aNewNameIsPublishedWhetherThePcOrThePersonGaveIt() {
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower 2"));
        assertEquals("Tower 2", label("tower"));

        ComputerDetails aliased = pc("tower", "Tower 2");
        aliased.alias = "Den";
        helper.createAppViewShortcutForOnlineHost(aliased);
        assertEquals("Den", label("tower"));

        helper.createAppViewShortcutForOnlineHost(aliased);
        assertEquals(0, asked());
        // The alias cleared: the PC's own name is back.
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower 2"));
        assertEquals("Tower 2", label("tower"));
    }

    @Test
    public void eachPcIsRememberedOnItsOwn() {
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        asked();
        helper.createAppViewShortcutForOnlineHost(pc("den", "Tower"));
        assertTrue(asked() > 0);
        assertEquals("Tower", label("den"));
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        helper.createAppViewShortcutForOnlineHost(pc("den", "Tower"));
        assertEquals(0, asked());
    }

    @Test
    public void aShortcutRemovedElsewhereComesBackOncePollingStartsAgain() {
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        manager.removeDynamicShortcuts(Collections.singletonList("tower"));
        assertNull(label("tower"));

        helper.forgetPublished();
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        assertEquals("Tower", label("tower"));
    }

    @Test
    public void aDisabledPcIsPublishedAgainByItsNextPoll() {
        ComputerDetails tower = pc("tower", "Tower");
        helper.createAppViewShortcutForOnlineHost(tower);
        helper.disableComputerShortcut(tower, "gone");
        asked();

        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        assertTrue(asked() > 0);
        assertEquals("Tower", label("tower"));
    }

    @Test
    public void makingRoomForAnotherShortcutForgetsThemAll() {
        int limit = manager.getMaxShortcutCountPerActivity();
        List<ShortcutInfo> shortcuts = new ArrayList<>();
        for (int i = 0; i < limit - 1; i++) {
            shortcuts.add(new ShortcutInfo.Builder(activity, "pc-" + i)
                    .setShortLabel("PC " + i).setRank(i)
                    .setIntent(new Intent(Intent.ACTION_MAIN)).build());
        }
        manager.addDynamicShortcuts(shortcuts);
        // The last free place is the tower's, and opening another PC takes one of them away.
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        helper.createAppViewShortcut(pc("opened", "Opened"), true, false);
        asked();

        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        assertTrue("it may be the one that went", asked() > 0);
    }

    @Test
    public void withNoRoomThePcIsTriedAgainByEveryPoll() {
        int limit = manager.getMaxShortcutCountPerActivity();
        List<ShortcutInfo> shortcuts = new ArrayList<>();
        for (int i = 0; i < limit; i++) {
            shortcuts.add(new ShortcutInfo.Builder(activity, "pc-" + i)
                    .setShortLabel("PC " + i).setRank(i)
                    .setIntent(new Intent(Intent.ACTION_MAIN)).build());
        }
        manager.addDynamicShortcuts(shortcuts);
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        assertNull("a poll evicts nothing", label("tower"));

        manager.removeDynamicShortcuts(Collections.singletonList("pc-0"));
        asked();
        helper.createAppViewShortcutForOnlineHost(pc("tower", "Tower"));
        assertEquals("Tower", label("tower"));
    }
}
