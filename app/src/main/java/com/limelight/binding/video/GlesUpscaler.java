package com.limelight.binding.video;

import android.content.Context;
import android.graphics.SurfaceTexture;
import android.opengl.EGL14;
import android.opengl.EGLConfig;
import android.opengl.EGLContext;
import android.opengl.EGLDisplay;
import android.opengl.EGLExt;
import android.opengl.EGLSurface;
import android.opengl.GLES11Ext;
import android.opengl.GLES30;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Process;
import android.view.Choreographer;
import android.view.Surface;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;

import java.io.IOException;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.concurrent.CountDownLatch;

final class GlesUpscaler implements AutoCloseable, SurfaceTexture.OnFrameAvailableListener {
    private final HandlerThread thread = new HandlerThread("Video - Upscaling", Process.THREAD_PRIORITY_DISPLAY);
    private final Handler handler;
    private final UpscalingPolicy policy;
    private final UpscalingPolicy.Mode mode;
    private final Packed444 packing;
    private final boolean tenBit;
    private final boolean stretch;
    private boolean hdr;
    private byte[] hdrMetadata;
    private final UpscalingFrameQueue frames = new UpscalingFrameQueue();
    private final long[] frame = new long[4];
    private final float[] transform = new float[16];
    private final int[] size = new int[2];
    // The stream's part of the decoded picture (UpscalingPolicy.pictureCrop), from the decoder's
    // output format: AV1 from AMD encoders keeps its alignment padding.
    private volatile float[] pictureCrop = {1f, 1f, 0f, 0f};
    private final Runnable pollCompletion = this::pollCompletion;
    private EGLDisplay display = EGL14.EGL_NO_DISPLAY;
    private EGLContext eglContext = EGL14.EGL_NO_CONTEXT;
    private EGLSurface window = EGL14.EGL_NO_SURFACE;
    private SurfaceTexture texture;
    private Surface inputSurface;
    private int sourceTexture, intermediateTexture, framebuffer;
    private int upscaleProgram, rcasProgram, transformUniform;
    private int surfaceWidth, surfaceHeight;
    private int[] viewport;
    private long fence;
    // workStartNs is when the decoder released the frame (for the stall check); gpuStartNs is when
    // it was latched, so the sample is the reconstruction and not the wait in front of it.
    private volatile long workStartNs;
    private long gpuStartNs;
    private int pendingDrops;
    private boolean frameAvailable;
    // glGetError synchronises with the driver, so release builds check it on every 120th draw only.
    private static final int ERROR_CHECK_INTERVAL = 120;
    private static final long FENCE_WAIT_NS = 4_000_000L;
    private int drawnFrames;
    private volatile boolean closed;
    private volatile String failureDetail;
    // Frame Warp (docs/frame-warp.md): the picture drawn without upscaling, shifted by warp.frag. The fields below
    // are the GL thread's.
    private final boolean frameWarp;
    private final int pictureWidth, pictureHeight;
    private final float[] warpOut = new float[4];
    private final int[] maskSize = new int[2];
    private byte[] maskCells = new byte[0];
    private ByteBuffer maskUpload;
    private int warpUniform, maskTexture;
    private int maskGeneration = -1;
    // The decoder time of the picture on screen (-1 before one), and the shift it was last drawn with, picture px.
    private long shownPtsUs = -1;
    private float drawnShiftX, drawnShiftY;
    private Choreographer choreographer;
    private final Choreographer.FrameCallback warpRefresh = this::onRefresh;
    // A redraw between pictures moves the shift by at least this much, picture px.
    static final float REDRAW_SHIFT_PX = 0.25f;

    GlesUpscaler(Context context, Surface output, UpscalingPolicy.Mode mode, int width, int height,
                 boolean stretch, int sharpness, UpscalingPolicy policy) {
        this(context, output, mode, width, height, stretch, sharpness, policy, null, false, null);
    }

    GlesUpscaler(Context context, Surface output, UpscalingPolicy.Mode mode, int width, int height,
                 boolean stretch, int sharpness, UpscalingPolicy policy, Packed444 packing,
                 boolean tenBit, byte[] hdrMetadata) {
        this(context, output, mode, width, height, stretch, sharpness, policy, packing, tenBit, hdrMetadata, false);
    }

    /** With {@code frameWarp}, an SDR presenter that draws Frame Warp with bilinear sampling and no upscaling. */
    GlesUpscaler(Context context, Surface output, UpscalingPolicy.Mode mode, int width, int height,
                 boolean stretch, int sharpness, UpscalingPolicy policy, Packed444 packing,
                 boolean tenBit, byte[] hdrMetadata, boolean frameWarp) {
        this.frameWarp = frameWarp && packing == null && !tenBit;
        this.pictureWidth = width;
        this.pictureHeight = height;
        this.mode = this.frameWarp ? UpscalingPolicy.Mode.BILINEAR : mode;
        this.policy = policy;
        this.packing = packing;
        this.tenBit = tenBit;
        this.hdr = tenBit;
        this.stretch = stretch;
        this.hdrMetadata = hdrMetadata == null ? null : hdrMetadata.clone();
        thread.start();
        handler = new Handler(thread.getLooper());
        CountDownLatch ready = new CountDownLatch(1);
        handler.post(() -> {
            try {
                initialize(context, output, width, height, stretch, sharpness);
            } catch (IOException | RuntimeException e) {
                LimeLog.warning("Upscaling initialization failed: " + e);
                failGpu(e);
            } finally {
                ready.countDown();
            }
        });
        boolean interrupted = false;
        while (true) {
            try {
                ready.await();
                break;
            } catch (InterruptedException e) {
                interrupted = true;
            }
        }
        if (interrupted) Thread.currentThread().interrupt();
    }

    Surface getInputSurface() {
        return inputSurface;
    }

    /** Whether a cropped edge ends half a texel inside: bilinear sampling, which does not clamp. */
    boolean insetsCroppedEdges() {
        return packing == null && mode == UpscalingPolicy.Mode.BILINEAR;
    }

    void setPictureCrop(float[] crop) {
        pictureCrop = crop.clone();
    }

    long releaseFrame(long targetNs) {
        return frames.release(System.nanoTime(), targetNs);
    }

    /** As releaseFrame(long), with the decoder's presentation time of the frame for Frame Warp. */
    long releaseFrame(long targetNs, long ptsUs) {
        return frames.release(System.nanoTime(), targetNs, ptsUs);
    }

    /** Whether this presenter draws Frame Warp. */
    boolean drawsFrameWarp() {
        return frameWarp;
    }

    void setFrameRate(int fps) {
        // Keep hysteresis updates on the thread that records completed GPU work.
        handler.post(() -> policy.setFrameRate(fps));
    }

    void checkHealth() {
        long nowNs = System.nanoTime();
        long startNs = workStartNs;
        if (startNs != 0) policy.checkStall(nowNs - startNs);
        long oldestNs = frames.oldestReleaseNs();
        if (oldestNs != 0) policy.checkStall(nowNs - oldestNs);
    }

    private void initialize(Context context, Surface output, int width, int height,
                            boolean stretch, int sharpness) throws IOException {
        display = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY);
        require(display != EGL14.EGL_NO_DISPLAY, "EGL display");
        require(EGL14.eglInitialize(display, size, 0, size, 1), "EGL initialize");
        String eglExtensions = EGL14.eglQueryString(display, EGL14.EGL_EXTENSIONS);
        if (eglExtensions == null || !eglExtensions.contains("EGL_ANDROID_presentation_time")) {
            policy.fail(UpscalingPolicy.Reason.UNSUPPORTED);
            return;
        }
        int[] attributes = {
                EGL14.EGL_RENDERABLE_TYPE, 0x40, EGL14.EGL_SURFACE_TYPE, EGL14.EGL_WINDOW_BIT,
                EGL14.EGL_RED_SIZE, tenBit ? 10 : 8, EGL14.EGL_GREEN_SIZE, tenBit ? 10 : 8, EGL14.EGL_BLUE_SIZE, tenBit ? 10 : 8,
                EGL14.EGL_ALPHA_SIZE, 0, EGL14.EGL_NONE
        };
        EGLConfig[] configs = new EGLConfig[1];
        require(EGL14.eglChooseConfig(display, attributes, 0, configs, 0, 1, size, 0) && size[0] != 0,
                "GLES 3 config");
        if (tenBit) for (int channel : new int[] {EGL14.EGL_RED_SIZE, EGL14.EGL_GREEN_SIZE, EGL14.EGL_BLUE_SIZE}) {
            require(EGL14.eglGetConfigAttrib(display, configs[0], channel, size, 0) && size[0] >= 10,
                    "10-bit HDR color buffer");
        }
        eglContext = EGL14.eglCreateContext(display, configs[0], EGL14.EGL_NO_CONTEXT,
                new int[] {EGL14.EGL_CONTEXT_CLIENT_VERSION, 3, EGL14.EGL_NONE}, 0);
        require(eglContext != EGL14.EGL_NO_CONTEXT, "GLES 3 context");
        if (tenBit) require(eglExtensions.contains("EGL_EXT_gl_colorspace_bt2020_pq"), "BT.2020/PQ surface");
        window = EGL14.eglCreateWindowSurface(display, configs[0], output,
                tenBit ? new int[] {0x309D, 0x3340, EGL14.EGL_NONE} : new int[] {EGL14.EGL_NONE}, 0);
        require(window != EGL14.EGL_NO_SURFACE, "EGL window");
        require(EGL14.eglMakeCurrent(display, window, window, eglContext), "EGL make current");
        String extensions = GLES30.glGetString(GLES30.GL_EXTENSIONS);
        if (extensions == null || !extensions.contains("GL_OES_EGL_image_external_essl3")) {
            policy.fail(UpscalingPolicy.Reason.UNSUPPORTED);
            return;
        }
        if (packing != null) require(extensions.contains("GL_EXT_YUV_target"), "raw YUV sampling");
        require(EGL14.eglSwapInterval(display, 0), "EGL swap interval");
        querySurfaceSize();
        surfaceWidth = size[0];
        surfaceHeight = size[1];
        viewport = UpscalingPolicy.viewport(packing == null ? width : packing.width,
                packing == null ? height : packing.height, surfaceWidth, surfaceHeight, stretch);
        // Frame Warp draws at any size: it shifts the picture rather than upscaling it.
        if (packing == null && !frameWarp) policy.fail(UpscalingPolicy.unavailableReason(mode, width, height,
                viewport[2], viewport[3], false, false, true));
        if (policy.getReason() != UpscalingPolicy.Reason.NONE) return;

        String vertex = shaderSource(context, "upscale.vert");
        upscaleProgram = program(vertex, shaderSource(context, frameWarp ? "warp.frag" :
                packing != null ? "packed444.frag" : mode == UpscalingPolicy.Mode.FSR1 ? "easu.frag" :
                        mode == UpscalingPolicy.Mode.SGSR1 ? "sgsr.frag" : "bilinear.frag"));
        GLES30.glUseProgram(upscaleProgram);
        GLES30.glUniform1i(GLES30.glGetUniformLocation(upscaleProgram, "source"), 0);
        transformUniform = GLES30.glGetUniformLocation(upscaleProgram, "textureTransform");
        if (frameWarp) initializeFrameWarp();
        if (packing != null) {
            GLES30.glUniform1i(GLES30.glGetUniformLocation(upscaleProgram, "packingVersion"), packing.version);
            GLES30.glUniform4i(GLES30.glGetUniformLocation(upscaleProgram, "picture"),
                    packing.width, packing.height, packing.paddedWidth, packing.paddedHeight);
            updateHdr();
        }
        if (mode == UpscalingPolicy.Mode.FSR1) {
            float[] constants = FsrConstants.easu(width, height, width, height, viewport[2], viewport[3]);
            for (int i = 0; i < 4; i++) {
                GLES30.glUniform4fv(GLES30.glGetUniformLocation(upscaleProgram, "con" + i), 1, constants, i * 4);
            }
            rcasProgram = program(vertex, shaderSource(context, "rcas.frag"));
            GLES30.glUseProgram(rcasProgram);
            GLES30.glUniform1i(GLES30.glGetUniformLocation(rcasProgram, "source"), 0);
            GLES30.glUniform1f(GLES30.glGetUniformLocation(rcasProgram, "sharpness"), FsrConstants.rcas(sharpness));
            GLES30.glGenTextures(1, size, 0);
            intermediateTexture = size[0];
            GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, intermediateTexture);
            textureParameters(GLES30.GL_TEXTURE_2D, GLES30.GL_NEAREST);
            GLES30.glTexStorage2D(GLES30.GL_TEXTURE_2D, 1, GLES30.GL_RGBA8, viewport[2], viewport[3]);
            GLES30.glGenFramebuffers(1, size, 0);
            framebuffer = size[0];
            GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, framebuffer);
            GLES30.glFramebufferTexture2D(GLES30.GL_FRAMEBUFFER, GLES30.GL_COLOR_ATTACHMENT0,
                    GLES30.GL_TEXTURE_2D, intermediateTexture, 0);
            require(GLES30.glCheckFramebufferStatus(GLES30.GL_FRAMEBUFFER) == GLES30.GL_FRAMEBUFFER_COMPLETE,
                    "EASU framebuffer");
            GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, 0);
        } else if (mode == UpscalingPolicy.Mode.SGSR1) {
            GLES30.glUniform4fv(GLES30.glGetUniformLocation(upscaleProgram, "viewportInfo"), 1,
                    SgsrConstants.viewport(width, height), 0);
            GLES30.glUniform1f(GLES30.glGetUniformLocation(upscaleProgram, "edgeSharpness"),
                    SgsrConstants.edgeSharpness(sharpness));
        }
        GLES30.glGenTextures(1, size, 0);
        sourceTexture = size[0];
        GLES30.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, sourceTexture);
        textureParameters(GLES11Ext.GL_TEXTURE_EXTERNAL_OES,
                packing != null || mode == UpscalingPolicy.Mode.FSR1 ? GLES30.GL_NEAREST : GLES30.GL_LINEAR);
        texture = new SurfaceTexture(sourceTexture);
        texture.setDefaultBufferSize(width, height);
        texture.setOnFrameAvailableListener(this, handler);
        inputSurface = new Surface(texture);
        GLES30.glDisable(GLES30.GL_DITHER);
        GLES30.glClearColor(0f, 0f, 0f, 1f);
        require(GLES30.glGetError() == GLES30.GL_NO_ERROR, "GLES setup");
        if (frameWarp) {
            // This thread's looper: the redraws between pictures run here, with the draws.
            choreographer = Choreographer.getInstance();
            choreographer.postFrameCallback(warpRefresh);
        }
    }

    /** The HUD mask texture on unit 1, empty until the host sends one. */
    private void initializeFrameWarp() {
        warpUniform = GLES30.glGetUniformLocation(upscaleProgram, "warp");
        GLES30.glUniform1i(GLES30.glGetUniformLocation(upscaleProgram, "hudMask"), 1);
        GLES30.glUniform3f(warpUniform, 0f, 0f, 0f);
        GLES30.glGenTextures(1, size, 0);
        maskTexture = size[0];
        GLES30.glActiveTexture(GLES30.GL_TEXTURE1);
        GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, maskTexture);
        textureParameters(GLES30.GL_TEXTURE_2D, GLES30.GL_NEAREST);
        uploadMask(1, 1, ByteBuffer.allocateDirect(1));
        GLES30.glActiveTexture(GLES30.GL_TEXTURE0);
    }

    // To the texture bound on unit 1, which keeps the mask bound.
    private static void uploadMask(int width, int height, ByteBuffer cells) {
        GLES30.glPixelStorei(GLES30.GL_UNPACK_ALIGNMENT, 1);
        GLES30.glTexImage2D(GLES30.GL_TEXTURE_2D, 0, GLES30.GL_R8, width, height, 0, GLES30.GL_RED,
                GLES30.GL_UNSIGNED_BYTE, cells);
    }

    /** Takes a newer HUD mask, if the host sent one, onto the mask texture. */
    private void refreshMask() {
        int generation = MoonBridge.frameWarpMask(maskGeneration, maskSize, maskCells);
        if (generation == MoonBridge.FRAME_WARP_MASK_TOO_SMALL && maskSize[0] > 0 && maskSize[1] > 0) {
            maskCells = new byte[maskSize[0] * maskSize[1]];
            generation = MoonBridge.frameWarpMask(maskGeneration, maskSize, maskCells);
        }
        if (generation == MoonBridge.FRAME_WARP_MASK_CLEARED) {
            maskGeneration = -1;
        } else if (generation >= 0) {
            int count = maskSize[0] * maskSize[1];
            if (maskUpload == null || maskUpload.capacity() < count) maskUpload = ByteBuffer.allocateDirect(count);
            maskUpload.clear();
            maskUpload.put(maskCells, 0, count);
            maskUpload.flip();
            GLES30.glActiveTexture(GLES30.GL_TEXTURE1);
            GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, maskTexture);
            uploadMask(maskSize[0], maskSize[1], maskUpload);
            GLES30.glActiveTexture(GLES30.GL_TEXTURE0);
            maskGeneration = generation;
        }
    }

    /** Sets the shift for the picture with decoder time ptsUs, drawn now; the program is in use. */
    private void applyWarp(long ptsUs) {
        refreshMask();
        int code = MoonBridge.frameWarpForPts(ptsUs, warpOut);
        boolean active = code != MoonBridge.FRAME_WARP_INACTIVE;
        drawnShiftX = active ? warpOut[2] : 0f;
        drawnShiftY = active ? warpOut[3] : 0f;
        GLES30.glUniform3f(warpUniform, active ? warpOut[0] : 0f, active ? warpOut[1] : 0f,
                active && code >= 0 && code == maskGeneration ? 1f : 0f);
    }

    /** Whether a shift that moved from the one on screen to (x, y), picture px, needs drawing again. */
    static boolean needsRedraw(float drawnX, float drawnY, float x, float y) {
        return Math.hypot(x - drawnX, y - drawnY) >= REDRAW_SHIFT_PX;
    }

    /**
     * Each refresh between pictures: draws the picture on screen again when its shift moved, so a turn shows on every
     * refresh and not only on the host's frames. Never while a draw is pending or a new picture is on its way.
     */
    private void onRefresh(long frameTimeNanos) {
        if (closed || policy.getReason() != UpscalingPolicy.Reason.NONE) return;
        choreographer.postFrameCallback(warpRefresh);
        if (shownPtsUs < 0 || fence != 0 || frameAvailable || frames.oldestReleaseNs() != 0) return;
        try {
            // Peeked, so a refresh that draws nothing is not counted; the redraw takes its own shift (applyWarp)
            int code = MoonBridge.frameWarpPeekForPts(shownPtsUs, warpOut);
            boolean active = code != MoonBridge.FRAME_WARP_INACTIVE;
            if (!needsRedraw(drawnShiftX, drawnShiftY, active ? warpOut[2] : 0f, active ? warpOut[3] : 0f)) return;
            long nowNs = System.nanoTime();
            workStartNs = nowNs;
            gpuStartNs = nowNs;
            pendingDrops = 0;
            render(shownPtsUs, nowNs);
        } catch (RuntimeException e) {
            LimeLog.warning("Frame Warp redraw failed: " + e);
            failGpu(e);
        }
    }

    static String shaderSource(Context context, String name) throws IOException {
        try (InputStream stream = context.getAssets().open("shaders/" + name)) {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            byte[] buffer = new byte[4096];
            int count;
            while ((count = stream.read(buffer)) != -1) bytes.write(buffer, 0, count);
            return new String(bytes.toByteArray(), StandardCharsets.UTF_8);
        }
    }

    static void textureParameters(int target, int filter) {
        GLES30.glTexParameteri(target, GLES30.GL_TEXTURE_MIN_FILTER, filter);
        GLES30.glTexParameteri(target, GLES30.GL_TEXTURE_MAG_FILTER, filter);
        GLES30.glTexParameteri(target, GLES30.GL_TEXTURE_WRAP_S, GLES30.GL_CLAMP_TO_EDGE);
        GLES30.glTexParameteri(target, GLES30.GL_TEXTURE_WRAP_T, GLES30.GL_CLAMP_TO_EDGE);
    }

    private static int shader(int type, String source) {
        int shader = GLES30.glCreateShader(type);
        GLES30.glShaderSource(shader, source);
        GLES30.glCompileShader(shader);
        int[] status = new int[1];
        GLES30.glGetShaderiv(shader, GLES30.GL_COMPILE_STATUS, status, 0);
        if (status[0] == 0) {
            String log = GLES30.glGetShaderInfoLog(shader);
            GLES30.glDeleteShader(shader);
            throw new IllegalStateException("Upscaling shader: " + log);
        }
        return shader;
    }

    static int program(String vertexSource, String fragmentSource) {
        int vertex = shader(GLES30.GL_VERTEX_SHADER, vertexSource);
        int fragment = 0;
        int program = 0;
        try {
            fragment = shader(GLES30.GL_FRAGMENT_SHADER, fragmentSource);
            program = GLES30.glCreateProgram();
            GLES30.glAttachShader(program, vertex);
            GLES30.glAttachShader(program, fragment);
            GLES30.glLinkProgram(program);
            int[] status = new int[1];
            GLES30.glGetProgramiv(program, GLES30.GL_LINK_STATUS, status, 0);
            if (status[0] == 0) throw new IllegalStateException("Upscaling link: " + GLES30.glGetProgramInfoLog(program));
            return program;
        } catch (RuntimeException e) {
            if (program != 0) GLES30.glDeleteProgram(program);
            throw e;
        } finally {
            GLES30.glDeleteShader(vertex);
            if (fragment != 0) GLES30.glDeleteShader(fragment);
        }
    }

    /**
     * Whether a 10-bit BT.2020 PQ window surface can be created, the output packed HDR needs: the
     * same extension and 10-bit colour buffer initialize() requires, checked before HDR is asked for.
     */
    static boolean tenBitOutputSupported() {
        EGLDisplay display = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY);
        int[] version = new int[2];
        if (display == EGL14.EGL_NO_DISPLAY || !EGL14.eglInitialize(display, version, 0, version, 1)) return false;
        String extensions = EGL14.eglQueryString(display, EGL14.EGL_EXTENSIONS);
        return tenBitOutputSupported(extensions, tenBitConfigAvailable(display));
    }

    static boolean tenBitOutputSupported(String eglExtensions, boolean tenBitConfig) {
        return tenBitConfig && eglExtensions != null && eglExtensions.contains("EGL_EXT_gl_colorspace_bt2020_pq");
    }

    private static boolean tenBitConfigAvailable(EGLDisplay display) {
        int[] attributes = {
                EGL14.EGL_RENDERABLE_TYPE, 0x40, EGL14.EGL_SURFACE_TYPE, EGL14.EGL_WINDOW_BIT,
                EGL14.EGL_RED_SIZE, 10, EGL14.EGL_GREEN_SIZE, 10, EGL14.EGL_BLUE_SIZE, 10,
                EGL14.EGL_ALPHA_SIZE, 0, EGL14.EGL_NONE
        };
        EGLConfig[] configs = new EGLConfig[1];
        int[] count = new int[1];
        int[] value = new int[1];
        if (!EGL14.eglChooseConfig(display, attributes, 0, configs, 0, 1, count, 0) || count[0] == 0) return false;
        for (int channel : new int[] {EGL14.EGL_RED_SIZE, EGL14.EGL_GREEN_SIZE, EGL14.EGL_BLUE_SIZE}) {
            if (!EGL14.eglGetConfigAttrib(display, configs[0], channel, value, 0) || value[0] < 10) return false;
        }
        return true;
    }

    /** What failed, for the message after a GPU_ERROR: the step that threw, null for no GPU error. */
    String failureDetail() {
        return failureDetail;
    }

    private void failGpu(Exception e) {
        if (failureDetail == null) failureDetail = e.getMessage();
        policy.fail(UpscalingPolicy.Reason.GPU_ERROR);
    }

    private static void require(boolean success, String operation) {
        if (!success) throw new IllegalStateException(operation + " failed");
    }

    private void querySurfaceSize() {
        require(EGL14.eglQuerySurface(display, window, EGL14.EGL_WIDTH, size, 0), "EGL width");
        require(EGL14.eglQuerySurface(display, window, EGL14.EGL_HEIGHT, size, 1), "EGL height");
    }

    @Override
    public void onFrameAvailable(SurfaceTexture surfaceTexture) {
        if (closed || policy.getReason() != UpscalingPolicy.Reason.NONE) return;
        frameAvailable = true;
        try {
            handler.removeCallbacks(pollCompletion);
            pollCompletion();
            // A frame behind a pending draw waits for it here, up to 4 ms, rather than for the next 1 ms poll.
            awaitFence();
            if (fence != 0 || !frameAvailable || policy.getReason() != UpscalingPolicy.Reason.NONE) return;
            drawFrame();
        } catch (RuntimeException e) {
            LimeLog.warning("Upscaling frame failed: " + e);
            failGpu(e);
        }
    }

    private void drawFrame() {
        try {
            frameAvailable = false;
            texture.updateTexImage();
            if (!frames.consume(texture.getTimestamp(), frame)) return;
            workStartNs = frame[0];
            gpuStartNs = System.nanoTime();
            pendingDrops = (int) frame[2];
            querySurfaceSize();
            if (size[0] != surfaceWidth || size[1] != surfaceHeight) {
                if (packing == null && !frameWarp) {
                    policy.fail(UpscalingPolicy.Reason.SIZE_CHANGED);
                    return;
                }
                surfaceWidth = size[0];
                surfaceHeight = size[1];
                viewport = UpscalingPolicy.viewport(packing != null ? packing.width : pictureWidth,
                        packing != null ? packing.height : pictureHeight, surfaceWidth, surfaceHeight, stretch);
            }
            texture.getTransformMatrix(transform);
            // A picture without padding samples SurfaceTexture's transform untouched.
            float[] crop = pictureCrop;
            if (crop[0] != 1f || crop[1] != 1f) UpscalingPolicy.applyCrop(transform, crop);
            if (frameWarp) shownPtsUs = frame[3];
            render(frame[3], frame[1]);
        } catch (RuntimeException e) {
            LimeLog.warning("Upscaling frame failed: " + e);
            failGpu(e);
        }
    }

    /** Draws the latched picture and presents it at presentNs; ptsUs is its decoder time, for Frame Warp. */
    private void render(long ptsUs, long presentNs) {
        GLES30.glActiveTexture(GLES30.GL_TEXTURE0);
        GLES30.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, sourceTexture);
        GLES30.glUseProgram(upscaleProgram);
        GLES30.glUniformMatrix4fv(transformUniform, 1, false, transform, 0);
        if (frameWarp) applyWarp(ptsUs);
        GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, framebuffer);
        if (mode == UpscalingPolicy.Mode.FSR1) {
            GLES30.glViewport(0, 0, viewport[2], viewport[3]);
            GLES30.glDrawArrays(GLES30.GL_TRIANGLES, 0, 3);
            GLES30.glBindFramebuffer(GLES30.GL_FRAMEBUFFER, 0);
            GLES30.glUseProgram(rcasProgram);
            GLES30.glBindTexture(GLES30.GL_TEXTURE_2D, intermediateTexture);
        }
        GLES30.glClear(GLES30.GL_COLOR_BUFFER_BIT);
        GLES30.glViewport(viewport[0], viewport[1], viewport[2], viewport[3]);
        GLES30.glDrawArrays(GLES30.GL_TRIANGLES, 0, 3);
        if (BuildConfig.DEBUG || ++drawnFrames % ERROR_CHECK_INTERVAL == 0) {
            require(GLES30.glGetError() == GLES30.GL_NO_ERROR, "Upscaling draw");
        }
        fence = GLES30.glFenceSync(GLES30.GL_SYNC_GPU_COMMANDS_COMPLETE, 0);
        require(fence != 0, "GPU completion fence");
        require(EGLExt.eglPresentationTimeANDROID(display, window, presentNs), "EGL presentation time");
        require(EGL14.eglSwapBuffers(display, window), "EGL swap");
        GLES30.glFlush();
        pollCompletion();
    }

    private void pollCompletion() {
        if (closed || fence == 0 || policy.getReason() != UpscalingPolicy.Reason.NONE) return;
        checkFence(GLES30.glClientWaitSync(fence, 0, 0));
    }

    /** Waits up to {@link #FENCE_WAIT_NS} on this thread for the pending draw, then completes it as the poll would. */
    private void awaitFence() {
        if (closed || fence == 0 || policy.getReason() != UpscalingPolicy.Reason.NONE) return;
        checkFence(GLES30.glClientWaitSync(fence, GLES30.GL_SYNC_FLUSH_COMMANDS_BIT, FENCE_WAIT_NS));
    }

    private void checkFence(int status) {
        long elapsed = System.nanoTime() - workStartNs;
        if (status == GLES30.GL_ALREADY_SIGNALED || status == GLES30.GL_CONDITION_SATISFIED) {
            GLES30.glDeleteSync(fence);
            fence = 0;
            workStartNs = 0;
            // From the latch to the fence: the draw, swap blocking and up to one wait or polling interval,
            // but not the wait for the render thread before the latch. A conservative estimate of
            // the reconstruction, not a scanout timestamp; the policy only acts on a sustained trend.
            policy.recordFrame(System.nanoTime() - gpuStartNs, pendingDrops);
            // Latch the newest waiting texture only after the previous draw has completed.
            if (frameAvailable && policy.getReason() == UpscalingPolicy.Reason.NONE) drawFrame();
        } else if (status == GLES30.GL_WAIT_FAILED) {
            if (failureDetail == null) failureDetail = "GPU fence wait failed";
            policy.fail(UpscalingPolicy.Reason.GPU_ERROR);
        } else {
            policy.checkStall(elapsed);
            if (policy.getReason() == UpscalingPolicy.Reason.NONE) handler.postDelayed(pollCompletion, 1);
        }
    }

    void setHdrMode(boolean enabled, byte[] metadata) {
        byte[] copy = metadata == null ? null : metadata.clone();
        handler.post(() -> {
            try {
                hdr = enabled;
                hdrMetadata = copy;
                updateHdr();
            } catch (RuntimeException e) {
                LimeLog.warning("Packed chroma HDR output failed: " + e);
                failGpu(e);
            }
        });
    }

    private void updateHdr() {
        require(!hdr || tenBit, "10-bit HDR output");
        GLES30.glUseProgram(upscaleProgram);
        GLES30.glUniform4i(GLES30.glGetUniformLocation(upscaleProgram, "colorimetry"),
                tenBit ? 1 : 0, packing.colorRange, hdr ? 2 : packing.colorSpace, hdr ? 1 : 0);
        if (!tenBit) return;
        String extensions = EGL14.eglQueryString(display, EGL14.EGL_EXTENSIONS);
        ByteBuffer metadata = hdr && hdrMetadata != null && hdrMetadata.length >= 24 ?
                ByteBuffer.wrap(hdrMetadata).order(ByteOrder.LITTLE_ENDIAN) : null;
        if (extensions.contains("EGL_EXT_surface_SMPTE2086_metadata")) {
            for (int i = 0; i < 10; i++) {
                int value = -1; // EGL_DONT_CARE is absent from Android's EGL14 bindings.
                if (metadata != null) {
                    value = metadata.getShort(i * 2) & 0xffff;
                    // Control metadata stores maximum luminance in nits and minimum in 0.0001 nits.
                    if (i == 8) value *= 50000;
                    if (i == 9) value *= 5;
                }
                require(EGL14.eglSurfaceAttrib(display, window, 0x3341 + i, value), "HDR mastering metadata");
            }
        }
        if (extensions.contains("EGL_EXT_surface_CTA861_3_metadata")) {
            for (int i = 0; i < 2; i++) require(EGL14.eglSurfaceAttrib(display, window, 0x3360 + i,
                    metadata == null ? -1 : (metadata.getShort(20 + i * 2) & 0xffff) * 50000),
                    "HDR content metadata");
        }
    }

    @Override
    public void close() {
        closed = true;
        handler.post(() -> {
            handler.removeCallbacksAndMessages(null);
            if (choreographer != null) choreographer.removeFrameCallback(warpRefresh);
            if (inputSurface != null) inputSurface.release();
            if (texture != null) texture.release();
            if (display != EGL14.EGL_NO_DISPLAY) {
                if (fence != 0) GLES30.glDeleteSync(fence);
                // The context owns the programs, textures and framebuffer.
                EGL14.eglMakeCurrent(display, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT);
                if (window != EGL14.EGL_NO_SURFACE) EGL14.eglDestroySurface(display, window);
                if (eglContext != EGL14.EGL_NO_CONTEXT) EGL14.eglDestroyContext(display, eglContext);
                EGL14.eglTerminate(display);
                EGL14.eglReleaseThread();
            }
            thread.quit();
        });
        boolean interrupted = false;
        while (thread.isAlive()) {
            try {
                thread.join();
            } catch (InterruptedException e) {
                interrupted = true;
            }
        }
        if (interrupted) Thread.currentThread().interrupt();
    }
}
