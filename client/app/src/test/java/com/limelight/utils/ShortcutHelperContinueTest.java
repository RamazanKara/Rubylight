package com.limelight.utils;

import android.app.Activity;
import android.app.Application;
import android.content.pm.ShortcutInfo;
import android.content.pm.ShortcutManager;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.List;

import static org.junit.Assert.*;

/** Long-pressing the app icon offers "Continue <last game>" first, for one game. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ShortcutHelperContinueTest {
    private static ComputerDetails pc(String uuid) {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = uuid;
        computer.name = uuid;
        return computer;
    }

    @Test
    public void theLastGameComesFirstAndReplacesThePreviousOne() {
        Activity activity = Robolectric.buildActivity(Activity.class).setup().get();
        ShortcutManager manager = activity.getSystemService(ShortcutManager.class);
        ShortcutHelper helper = new ShortcutHelper(activity);

        helper.createAppViewShortcut(pc("desk"), true, false);
        helper.reportContinue(pc("desk"), new NvApp("Dota 2", 7, false));
        helper.reportContinue(pc("desk"), new NvApp("Hades", 9, false));
        helper.reportContinue(pc("lounge"), new NvApp("Celeste", 3, false));

        List<ShortcutInfo> shortcuts = manager.getDynamicShortcuts();
        ShortcutInfo cont = null;
        int continues = 0;
        for (ShortcutInfo info : shortcuts) {
            if (info.getId().endsWith(":continue")) {
                cont = info;
                continues++;
            }
        }
        assertEquals("one game, the last", 1, continues);
        assertEquals("Continue Celeste", cont.getShortLabel().toString());
        assertEquals(0, cont.getRank());
        assertEquals("lounge", cont.getIntent().getStringExtra(com.limelight.AppView.UUID_EXTRA));
    }
}
