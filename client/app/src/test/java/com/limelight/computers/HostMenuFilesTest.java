package com.limelight.computers;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.grid.PcGridAdapter;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/** When a PC's menu offers Files: a paired PC that is on and whose host speaks the native API. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostMenuFilesTest {
    @Test
    public void aReadyOrRunningNativeHostOffersFiles() {
        ComputerDetails tower = NativeApiFixtures.nativeHost("tower", "Tower", true);
        assertTrue(HostMenu.offersFiles(tower, PcGridAdapter.RowState.READY));
        assertTrue(HostMenu.offersFiles(tower, PcGridAdapter.RowState.RUNNING));
    }

    @Test
    public void anOfflineOrUnpairedPcHasNoFiles() {
        ComputerDetails tower = NativeApiFixtures.nativeHost("tower", "Tower", false);
        assertFalse(HostMenu.offersFiles(tower, PcGridAdapter.RowState.OFFLINE));
        assertFalse(HostMenu.offersFiles(tower, PcGridAdapter.RowState.NOT_PAIRED));
    }

    @Test
    public void aHostWithoutTheNativeApiNeverShowsFiles() {
        // Sunshine, Apollo and Vibepollo have no file transfer, nor a Rubylight host that has not said it does.
        for (String fixture : new String[] {"sunshine-serverinfo.xml", "apollo-serverinfo.xml",
                "vibepollo-serverinfo.xml", "rubylight-serverinfo.xml"}) {
            ComputerDetails host = NativeApiFixtures.host("pc", "PC", TestHosts.fixture(fixture), true);
            assertFalse(fixture, HostMenu.offersFiles(host, PcGridAdapter.RowState.READY));
        }
    }
}
