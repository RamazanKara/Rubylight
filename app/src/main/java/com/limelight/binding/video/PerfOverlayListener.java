package com.limelight.binding.video;

public interface PerfOverlayListener {
    void onPerfUpdate(String video, String network, String decode, CharSequence statisticsText,
                      CharSequence compactText);
}
