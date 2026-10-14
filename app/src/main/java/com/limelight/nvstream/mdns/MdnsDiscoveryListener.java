package com.limelight.nvstream.mdns;

public interface MdnsDiscoveryListener {
    /** Returns true once the host answered; false lets a later discovery session report it again. */
    boolean notifyComputerAdded(MdnsComputer computer);
    void notifyDiscoveryFailure(Exception e);
}
