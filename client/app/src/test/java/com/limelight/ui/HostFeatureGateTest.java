package com.limelight.ui;

import android.app.Application;

import com.limelight.R;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static com.limelight.nvstream.http.HostCapabilities.Feature.*;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostFeatureGateTest {
    private static HostFeatureGate gate(String fixture, HostCapabilities.Feature feature) {
        return HostFeatureGate.of(TestHosts.fixture(fixture), feature);
    }

    @Test
    public void displayModePickerOnlyForHostsWithDisplayModes() {
        for (String host : new String[] {"sunshine", "apollo", "vibepollo", "gamestream"}) {
            assertEquals(host, HostFeatureGate.Presentation.HIDDEN, gate(host + "-serverinfo.xml", HOST_DISPLAY_MODES).presentation);
        }
        HostFeatureGate rubylight = gate("rubylight-serverinfo.xml", HOST_DISPLAY_MODES);
        assertTrue(rubylight.isEnabled());
        assertEquals(0, rubylight.note);
        HostFeatureGate older = gate("butterpollo-serverinfo.xml", HOST_DISPLAY_MODES);
        assertTrue(older.isVisible());
        assertFalse(older.isEnabled());
        assertEquals(R.string.host_feature_reason_update, older.note);
    }

    @Test
    public void virtualDisplaySwitchForHostsWithAVirtualDisplay() {
        for (String host : new String[] {"apollo", "vibepollo", "rubylight", "butterpollo"}) {
            assertTrue(host, gate(host + "-serverinfo.xml", VIRTUAL_DISPLAY).isEnabled());
        }
        for (String host : new String[] {"sunshine", "gamestream"}) {
            assertFalse(host, gate(host + "-serverinfo.xml", VIRTUAL_DISPLAY).isVisible());
        }
        HostFeatureGate off = HostFeatureGate.of(TestHosts.caps(TestHosts.serverInfo("rubylight-serverinfo.xml")
                .replace("<VirtualDisplayCapable>true", "<VirtualDisplayCapable>false")), VIRTUAL_DISPLAY);
        assertEquals(HostFeatureGate.Presentation.DISABLED, off.presentation);
        assertEquals(R.string.host_feature_reason_virtual_display_off, off.note);
    }

    @Test
    public void neverSeenPcsKeepControlsWithANote() {
        for (HostCapabilities.Feature feature : HostCapabilities.Feature.values()) {
            HostFeatureGate gate = HostFeatureGate.of(HostCapabilities.UNKNOWN, feature);
            assertTrue(feature.name(), gate.isEnabled());
            assertEquals(feature.name(), R.string.host_feature_unknown, gate.note);
        }
    }

    @Test
    public void notesFollowTheControlsOwnText() {
        Application app = RuntimeEnvironment.getApplication();
        HostFeatureGate older = gate("butterpollo-serverinfo.xml", HOST_DISPLAY_MODES);
        assertEquals("PC display mode\nUpdate the host software to use this",
                older.withNote(app, "PC display mode").toString());
        assertEquals("Update the host software to use this", older.withNote(app, null).toString());
        assertEquals("PC display mode", gate("rubylight-serverinfo.xml", HOST_DISPLAY_MODES).withNote(app, "PC display mode"));
    }
}
