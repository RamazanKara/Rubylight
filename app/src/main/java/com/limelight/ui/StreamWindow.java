package com.limelight.ui;

/**
 * An activity that shows a stream full screen. AdaptiveLayout asks it which safe area the video
 * keeps: a full-panel stream (native, follow-screen or stretched) spans the whole display,
 * camera cutout included; the cutout-safe presets and windowed streams keep their insets.
 */
public interface StreamWindow {
    /** True when the video should cover the whole panel, cutout included. */
    boolean usesFullDisplayForVideo();

    /**
     * True in split screen or a free-form window smaller than the display. A multi-window
     * activity whose window still covers the whole display streams like a full-screen one.
     */
    boolean isWindowedStream();
}
