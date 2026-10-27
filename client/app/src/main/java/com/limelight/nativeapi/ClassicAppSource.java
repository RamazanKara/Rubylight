package com.limelight.nativeapi;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import java.util.List;

/** The Moonlight app list of a host that has no native API. Never touches the network. */
public interface ClassicAppSource {
    /** The apps the host last listed, or null when this device has never read them. */
    List<NvApp> apps(ComputerDetails computer);
}
