package com.limelight.utils;

import android.app.Activity;
import android.app.Application;
import android.content.Intent;
import android.content.pm.ShortcutInfo;
import android.content.pm.ShortcutManager;

import com.limelight.nvstream.http.ComputerDetails;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowShortcutManager;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class, shadows = ShortcutLifecycleTest.BoundedShortcutManager.class)
public class ShortcutLifecycleTest {
    @Implements(ShortcutManager.class)
    public static class BoundedShortcutManager extends ShadowShortcutManager {
        private int removals;

        @Implementation
        protected void removeDynamicShortcuts(List<String> ids) {
            if (++removals > 5) {
                throw new AssertionError("Shortcut eviction is repeating without making progress");
            }
            super.removeDynamicShortcuts(ids);
        }
    }

    @Test
    public void openingAnotherComputerAtTheShortcutLimitEvictsOnlyTheLastRank() {
        Activity activity = Robolectric.buildActivity(Activity.class).create().get();
        ShortcutManager manager = activity.getSystemService(ShortcutManager.class);
        int limit = manager.getMaxShortcutCountPerActivity();
        List<ShortcutInfo> shortcuts = new ArrayList<>();
        for (int i = 0; i < limit; i++) {
            shortcuts.add(new ShortcutInfo.Builder(activity, "pc-" + i)
                    .setShortLabel("PC " + i).setRank(i)
                    .setIntent(new Intent(Intent.ACTION_MAIN)).build());
        }
        manager.addDynamicShortcuts(shortcuts);
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = "new-pc";
        computer.name = "New PC";

        new ShortcutHelper(activity).createAppViewShortcut(computer, true, false);

        assertEquals(limit, manager.getDynamicShortcuts().size());
        assertTrue(manager.getDynamicShortcuts().stream().anyMatch(s -> s.getId().equals("new-pc")));
        assertFalse(manager.getDynamicShortcuts().stream().anyMatch(s -> s.getId().equals("pc-" + (limit - 1))));
        activity.finish();
    }
}
