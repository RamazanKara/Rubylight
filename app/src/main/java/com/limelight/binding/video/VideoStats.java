package com.limelight.binding.video;

import android.os.SystemClock;

class VideoStats {

    int totalFrames;
    int totalFramesReceived;
    int totalFramesRendered;
    int frameLossEvents;
    int framesLost;
    // Whole milliseconds of decode time counted the Artemis way (see FrameLatencyStats).
    long decoderTimeMs;
    char minHostProcessingLatency;
    char maxHostProcessingLatency;
    long totalHostProcessingLatency;
    int framesWithHostProcessingLatency;
    long measurementStartTimestamp;

    void add(VideoStats other) {
        this.totalFrames += other.totalFrames;
        this.totalFramesReceived += other.totalFramesReceived;
        this.totalFramesRendered += other.totalFramesRendered;
        this.frameLossEvents += other.frameLossEvents;
        this.framesLost += other.framesLost;
        this.decoderTimeMs += other.decoderTimeMs;

        if (this.minHostProcessingLatency == 0) {
            this.minHostProcessingLatency = other.minHostProcessingLatency;
        } else if (other.minHostProcessingLatency != 0) {
            this.minHostProcessingLatency = (char) Math.min(this.minHostProcessingLatency, other.minHostProcessingLatency);
        }
        this.maxHostProcessingLatency = (char) Math.max(this.maxHostProcessingLatency, other.maxHostProcessingLatency);
        this.totalHostProcessingLatency += other.totalHostProcessingLatency;
        this.framesWithHostProcessingLatency += other.framesWithHostProcessingLatency;

        if (this.measurementStartTimestamp == 0) {
            this.measurementStartTimestamp = other.measurementStartTimestamp;
        }

        assert other.measurementStartTimestamp >= this.measurementStartTimestamp;
    }

    void copy(VideoStats other) {
        this.totalFrames = other.totalFrames;
        this.totalFramesReceived = other.totalFramesReceived;
        this.totalFramesRendered = other.totalFramesRendered;
        this.frameLossEvents = other.frameLossEvents;
        this.framesLost = other.framesLost;
        this.decoderTimeMs = other.decoderTimeMs;
        this.minHostProcessingLatency = other.minHostProcessingLatency;
        this.maxHostProcessingLatency = other.maxHostProcessingLatency;
        this.totalHostProcessingLatency = other.totalHostProcessingLatency;
        this.framesWithHostProcessingLatency = other.framesWithHostProcessingLatency;
        this.measurementStartTimestamp = other.measurementStartTimestamp;
    }

    void clear() {
        this.totalFrames = 0;
        this.totalFramesReceived = 0;
        this.totalFramesRendered = 0;
        this.frameLossEvents = 0;
        this.framesLost = 0;
        this.decoderTimeMs = 0;
        this.minHostProcessingLatency = 0;
        this.maxHostProcessingLatency = 0;
        this.totalHostProcessingLatency = 0;
        this.framesWithHostProcessingLatency = 0;
        this.measurementStartTimestamp = 0;
    }

    VideoStatsFps getFps() {
        float elapsed = (SystemClock.uptimeMillis() - this.measurementStartTimestamp) / (float) 1000;

        VideoStatsFps fps = new VideoStatsFps();
        fps.lossPercent = totalFrames == 0 ? -1 : (float) framesLost / totalFrames * 100;
        if (elapsed > 0) {
            fps.totalFps = this.totalFrames / elapsed;
            fps.receivedFps = this.totalFramesReceived / elapsed;
            fps.renderedFps = this.totalFramesRendered / elapsed;
        }
        return fps;
    }
}

class VideoStatsFps {

    float lossPercent;
    float totalFps;
    float receivedFps;
    float renderedFps;
}
