package com.limelight.utils;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;

public class ShortcutHelperTest {
    private static ComputerDetails computer(String uuid) {
        ComputerDetails computer = new ComputerDetails();
        computer.uuid = uuid;
        return computer;
    }

    @Test
    public void stockAppsKeepLegacyShortcutIds() {
        assertEquals("host-uuid42", ShortcutHelper.getShortcutIdForGame(
                computer("host-uuid"), new NvApp("Game", 42, false)));
    }

    @Test
    public void appUuidRemainsStableWhenNumericIdChanges() {
        NvApp app = new NvApp("Game", 42, false);
        app.setAppUuid("app-uuid");
        String original = ShortcutHelper.getShortcutIdForGame(computer("host-uuid"), app);
        app.setAppId(99);
        assertEquals(original, ShortcutHelper.getShortcutIdForGame(computer("host-uuid"), app));
    }

    @Test
    public void reusedNumericIdsDoNotOverwriteAnotherAppShortcut() {
        NvApp first = new NvApp("First", 42, false);
        NvApp second = new NvApp("Second", 42, false);
        first.setAppUuid("first-uuid");
        second.setAppUuid("second-uuid");
        assertNotEquals(ShortcutHelper.getShortcutIdForGame(computer("host-uuid"), first),
                ShortcutHelper.getShortcutIdForGame(computer("host-uuid"), second));
    }

    @Test
    public void appUuidsAreScopedToHostAndSeparateFromNumericIds() {
        NvApp app = new NvApp("Game", 42, false);
        app.setAppUuid("42");
        String firstHost = ShortcutHelper.getShortcutIdForGame(computer("host-one"), app);
        assertNotEquals(firstHost, ShortcutHelper.getShortcutIdForGame(computer("host-two"), app));
        app.setAppUuid(null);
        assertNotEquals(firstHost, ShortcutHelper.getShortcutIdForGame(computer("host-one"), app));
    }
}
