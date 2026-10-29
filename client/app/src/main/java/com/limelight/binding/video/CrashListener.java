package com.limelight.binding.video;

/** Told once per renderer when it gives the decoder up and ends the stream (-107). */
public interface CrashListener {
    void notifyCrash(Exception e);
}
