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
import android.view.Surface;

import com.limelight.LimeLog;

import java.io.IOException;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.CountDownLatch;

final class GlesUpscaler implements AutoCloseable, SurfaceTexture.OnFrameAvailableListener {
    private final HandlerThread thread = new HandlerThread("Video - Upscaling", Process.THREAD_PRIORITY_DISPLAY);
    private final Handler handler;
    private final UpscalingPolicy policy;
    private final UpscalingPolicy.Mode mode;
    private final UpscalingFrameQueue frames = new UpscalingFrameQueue();
    private final long[] frame = new long[3];
    private final float[] transform = new float[16];
    private final int[] size = new int[2];
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
    private volatile long workStartNs;
    private int pendingDrops;
    private boolean frameAvailable;
    private volatile boolean closed;

    GlesUpscaler(Context context, Surface output, UpscalingPolicy.Mode mode, int width, int height,
                 boolean stretch, int sharpness, UpscalingPolicy policy) {
        this.mode = mode;
        this.policy = policy;
        thread.start();
        handler = new Handler(thread.getLooper());
        CountDownLatch ready = new CountDownLatch(1);
        handler.post(() -> {
            try {
                initialize(context, output, width, height, stretch, sharpness);
            } catch (IOException | RuntimeException e) {
                LimeLog.warning("Upscaling initialization failed: " + e);
                policy.fail(UpscalingPolicy.Reason.GPU_ERROR);
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

    long releaseFrame(long targetNs) {
        return frames.release(System.nanoTime(), targetNs);
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
                EGL14.EGL_RED_SIZE, 8, EGL14.EGL_GREEN_SIZE, 8, EGL14.EGL_BLUE_SIZE, 8,
                EGL14.EGL_ALPHA_SIZE, 0, EGL14.EGL_NONE
        };
        EGLConfig[] configs = new EGLConfig[1];
        require(EGL14.eglChooseConfig(display, attributes, 0, configs, 0, 1, size, 0) && size[0] != 0,
                "GLES 3 config");
        eglContext = EGL14.eglCreateContext(display, configs[0], EGL14.EGL_NO_CONTEXT,
                new int[] {EGL14.EGL_CONTEXT_CLIENT_VERSION, 3, EGL14.EGL_NONE}, 0);
        require(eglContext != EGL14.EGL_NO_CONTEXT, "GLES 3 context");
        window = EGL14.eglCreateWindowSurface(display, configs[0], output, new int[] {EGL14.EGL_NONE}, 0);
        require(window != EGL14.EGL_NO_SURFACE, "EGL window");
        require(EGL14.eglMakeCurrent(display, window, window, eglContext), "EGL make current");
        String extensions = GLES30.glGetString(GLES30.GL_EXTENSIONS);
        if (extensions == null || !extensions.contains("GL_OES_EGL_image_external_essl3")) {
            policy.fail(UpscalingPolicy.Reason.UNSUPPORTED);
            return;
        }
        require(EGL14.eglSwapInterval(display, 0), "EGL swap interval");
        querySurfaceSize();
        surfaceWidth = size[0];
        surfaceHeight = size[1];
        viewport = UpscalingPolicy.viewport(width, height, surfaceWidth, surfaceHeight, stretch);
        policy.fail(UpscalingPolicy.unavailableReason(mode, width, height, viewport[2], viewport[3],
                false, false, true));
        if (policy.getReason() != UpscalingPolicy.Reason.NONE) return;

        String vertex = shaderSource(context, "upscale.vert");
        upscaleProgram = program(vertex, shaderSource(context,
                mode == UpscalingPolicy.Mode.FSR1 ? "easu.frag" :
                        mode == UpscalingPolicy.Mode.SGSR1 ? "sgsr.frag" : "bilinear.frag"));
        GLES30.glUseProgram(upscaleProgram);
        GLES30.glUniform1i(GLES30.glGetUniformLocation(upscaleProgram, "source"), 0);
        transformUniform = GLES30.glGetUniformLocation(upscaleProgram, "textureTransform");
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
                mode == UpscalingPolicy.Mode.FSR1 ? GLES30.GL_NEAREST : GLES30.GL_LINEAR);
        texture = new SurfaceTexture(sourceTexture);
        texture.setDefaultBufferSize(width, height);
        texture.setOnFrameAvailableListener(this, handler);
        inputSurface = new Surface(texture);
        GLES30.glDisable(GLES30.GL_DITHER);
        GLES30.glClearColor(0f, 0f, 0f, 1f);
        require(GLES30.glGetError() == GLES30.GL_NO_ERROR, "GLES setup");
    }

    private static String shaderSource(Context context, String name) throws IOException {
        try (InputStream stream = context.getAssets().open("shaders/" + name)) {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            byte[] buffer = new byte[4096];
            int count;
            while ((count = stream.read(buffer)) != -1) bytes.write(buffer, 0, count);
            return new String(bytes.toByteArray(), StandardCharsets.UTF_8);
        }
    }

    private static void textureParameters(int target, int filter) {
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

    private static int program(String vertexSource, String fragmentSource) {
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
            if (fence != 0 || !frameAvailable || policy.getReason() != UpscalingPolicy.Reason.NONE) return;
            drawFrame();
        } catch (RuntimeException e) {
            LimeLog.warning("Upscaling frame failed: " + e);
            policy.fail(UpscalingPolicy.Reason.GPU_ERROR);
        }
    }

    private void drawFrame() {
        try {
            frameAvailable = false;
            texture.updateTexImage();
            if (!frames.consume(texture.getTimestamp(), frame)) return;
            workStartNs = frame[0];
            pendingDrops = (int) frame[2];
            querySurfaceSize();
            if (size[0] != surfaceWidth || size[1] != surfaceHeight) {
                policy.fail(UpscalingPolicy.Reason.SIZE_CHANGED);
                return;
            }
            texture.getTransformMatrix(transform);
            GLES30.glActiveTexture(GLES30.GL_TEXTURE0);
            GLES30.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, sourceTexture);
            GLES30.glUseProgram(upscaleProgram);
            GLES30.glUniformMatrix4fv(transformUniform, 1, false, transform, 0);
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
            require(GLES30.glGetError() == GLES30.GL_NO_ERROR, "Upscaling draw");
            fence = GLES30.glFenceSync(GLES30.GL_SYNC_GPU_COMMANDS_COMPLETE, 0);
            require(fence != 0, "GPU completion fence");
            require(EGLExt.eglPresentationTimeANDROID(display, window, frame[1]), "EGL presentation time");
            require(EGL14.eglSwapBuffers(display, window), "EGL swap");
            GLES30.glFlush();
            pollCompletion();
        } catch (RuntimeException e) {
            LimeLog.warning("Upscaling frame failed: " + e);
            policy.fail(UpscalingPolicy.Reason.GPU_ERROR);
        }
    }

    private void pollCompletion() {
        if (closed || fence == 0 || policy.getReason() != UpscalingPolicy.Reason.NONE) return;
        int status = GLES30.glClientWaitSync(fence, 0, 0);
        long elapsed = System.nanoTime() - workStartNs;
        if (status == GLES30.GL_ALREADY_SIGNALED || status == GLES30.GL_CONDITION_SATISFIED) {
            GLES30.glDeleteSync(fence);
            fence = 0;
            workStartNs = 0;
            // Includes handoff, GPU work, swap blocking and up to one polling interval.
            // This is a conservative frame-time estimate, not a scanout timestamp.
            policy.recordFrame(elapsed, pendingDrops);
            // Latch the newest waiting texture only after the previous draw has completed.
            if (frameAvailable && policy.getReason() == UpscalingPolicy.Reason.NONE) drawFrame();
        } else if (status == GLES30.GL_WAIT_FAILED) {
            policy.fail(UpscalingPolicy.Reason.GPU_ERROR);
        } else {
            policy.checkStall(elapsed);
            if (policy.getReason() == UpscalingPolicy.Reason.NONE) handler.postDelayed(pollCompletion, 1);
        }
    }

    @Override
    public void close() {
        closed = true;
        handler.post(() -> {
            handler.removeCallbacksAndMessages(null);
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
