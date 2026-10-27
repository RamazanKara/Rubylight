package com.limelight.computers;

import com.limelight.nvstream.http.ComputerDetails;

public interface ComputerManagerListener {
    void notifyComputerUpdated(ComputerDetails details);

    default void notifyAppListUpdated(ComputerDetails details, boolean success) {
        if (success) notifyComputerUpdated(details);
    }
}
