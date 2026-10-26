package com.limelight.policy;

import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.AdaptiveQualityController.Target;
import com.limelight.nvstream.DecodeTiming;

import java.util.ArrayList;
import java.util.List;

/**
 * The calls of the quality traces in plain values, as nightjar-policy's quality::Traced takes
 * them. Targets are "WIDTHxHEIGHT@FPS"; a timing is eight trailing arguments.
 */
final class AdaptiveQualityTrace {
    private final AdaptiveQualityController controller;
    // applied() takes only the Target objects the controller handed out.
    private final List<Target> handedOut = new ArrayList<>();

    AdaptiveQualityTrace(String mode, int width, int height, int fps, int minShortEdge,
                         int minFps, int alignWidth, int alignHeight, long now) {
        controller = new AdaptiveQualityController(AdaptiveQualityController.Mode.parse(mode),
                width, height, fps, minShortEdge, minFps, alignWidth, alignHeight, now);
    }

    static String parseMode(String value) {
        return AdaptiveQualityController.Mode.parse(value).preference();
    }

    String sample(long now, boolean recentVideo, boolean poorConnection, float loss, float decodeMs,
                  float jitterMs, float queueMs, int thermalLevel, boolean bitrateHasRoom) {
        return name(controller.sample(now, recentVideo, poorConnection, loss, decodeMs, jitterMs,
                queueMs, thermalLevel, bitrateHasRoom));
    }

    String sampleWithoutTiming(long now, boolean recentVideo, boolean poorConnection, float loss,
                               float decodeMs, float jitterMs, float queueMs, int thermalLevel,
                               boolean bitrateHasRoom) {
        return name(controller.sample(now, recentVideo, poorConnection, loss, decodeMs, jitterMs,
                queueMs, thermalLevel, bitrateHasRoom, null));
    }

    String sampleTimed(long now, boolean recentVideo, boolean poorConnection, float loss,
                       float decodeMs, float jitterMs, float queueMs, int thermalLevel,
                       boolean bitrateHasRoom, long generation, long windowStartNs, long windowEndNs,
                       long firstSampleNs, long lastSampleNs, int fps, int samples, int lateFrames) {
        DecodeTiming timing = new DecodeTiming(generation, windowStartNs, windowEndNs,
                firstSampleNs, lastSampleNs, fps, samples, lateFrames);
        return name(controller.sample(now, recentVideo, poorConnection, loss, decodeMs, jitterMs,
                queueMs, thermalLevel, bitrateHasRoom, timing));
    }

    void applied(String target, long now) {
        for (int i = handedOut.size() - 1; i >= 0; i--) {
            if (format(handedOut.get(i)).equals(target)) {
                controller.applied(handedOut.get(i), now);
                return;
            }
        }
        throw new AssertionError("the controller never returned " + target);
    }

    String resizeCeiling(int width, int height, int fps, long now) {
        return name(controller.resizeCeiling(width, height, fps, now));
    }

    void suspend() {
        controller.suspend();
    }

    String current() {
        return name(controller.current());
    }

    String ceiling() {
        return name(controller.ceiling());
    }

    private String name(Target target) {
        if (target == null) return null;
        handedOut.add(target);
        return format(target);
    }

    private static String format(Target target) {
        return target.width + "x" + target.height + "@" + target.fps;
    }
}
