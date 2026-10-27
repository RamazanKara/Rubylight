package com.limelight.computers;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.grid.PcGridAdapter;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/** Shared folders in a PC's menu: on a paired, online PC with the native API; the screen asks the PC the rest. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostMenuSharedFoldersTest {
    private static ComputerDetails pc(String nativeApiVersion) {
        ComputerDetails details = new ComputerDetails();
        details.uuid = "pc";
        details.caps = TestHosts.caps("<root status_code=\"200\"><RustHostVersion>3.0.0</RustHostVersion>"
                + "<PairStatus>1</PairStatus>" + (nativeApiVersion == null ? ""
                : "<NativeApiVersion>" + nativeApiVersion + "</NativeApiVersion>") + "</root>");
        return details;
    }

    @Test
    public void aReadyNativeApiPcOffersSharedFolders() {
        assertTrue(HostMenu.hasSharedFolders(pc("1"), PcGridAdapter.RowState.READY));
        assertTrue(HostMenu.hasSharedFolders(pc("1"), PcGridAdapter.RowState.RUNNING));
    }

    @Test
    public void anOfflineUnpairedOrOlderPcDoesNot() {
        assertFalse(HostMenu.hasSharedFolders(pc("1"), PcGridAdapter.RowState.OFFLINE));
        assertFalse(HostMenu.hasSharedFolders(pc("1"), PcGridAdapter.RowState.NOT_PAIRED));
        // Version 0 is what the host says to an unpaired certificate; no version is a host before the native API.
        assertFalse(HostMenu.hasSharedFolders(pc("0"), PcGridAdapter.RowState.READY));
        assertFalse(HostMenu.hasSharedFolders(pc(null), PcGridAdapter.RowState.READY));
        ComputerDetails sunshine = new ComputerDetails();
        sunshine.caps = TestHosts.fixture("sunshine-serverinfo.xml");
        assertFalse(HostMenu.hasSharedFolders(sunshine, PcGridAdapter.RowState.READY));
    }
}
