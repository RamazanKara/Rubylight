package com.limelight.binding.video;

import android.content.Context;
import android.graphics.SurfaceTexture;
import android.media.MediaCodec;
import android.media.MediaFormat;
import android.opengl.EGL14;
import android.opengl.EGLConfig;
import android.opengl.EGLContext;
import android.opengl.EGLDisplay;
import android.opengl.EGLSurface;
import android.opengl.GLES11Ext;
import android.opengl.GLES30;
import android.os.Handler;
import android.os.HandlerThread;
import android.view.Surface;

import com.limelight.LimeLog;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.nio.ByteBuffer;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

final class Packed444 {
    final int width, height, paddedWidth, paddedHeight, colorSpace, colorRange;

    Packed444(int width, int height, int colorSpace, int colorRange) {
        this.width = width;
        this.height = height;
        this.paddedWidth = (width + 63) & ~63;
        this.paddedHeight = (height + 7) & ~7;
        this.colorSpace = colorSpace;
        this.colorRange = colorRange;
    }

    // The four assets are one lossless 128x128 frame: Y=(x+13y)&max,
    // U=(7x+19y+17)&max, V=max-U. Check every code, including the low two bits.
    static boolean verifySampling(Context context, String decoder, String codec, boolean tenBit) {
        EGLDisplay display = EGL14.EGL_NO_DISPLAY;
        EGLContext eglContext = EGL14.EGL_NO_CONTEXT;
        EGLSurface pbuffer = EGL14.EGL_NO_SURFACE;
        SurfaceTexture texture = null;
        Surface surface = null;
        MediaCodec mediaCodec = null;
        HandlerThread callbacks = new HandlerThread("Packed chroma probe");
        callbacks.start();
        try {
            display = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY);
            int[] size = new int[2];
            require(EGL14.eglInitialize(display, size, 0, size, 1), "EGL initialization");
            EGLConfig[] configs = new EGLConfig[1];
            int[] attributes = {EGL14.EGL_RENDERABLE_TYPE, 0x40, EGL14.EGL_SURFACE_TYPE, EGL14.EGL_PBUFFER_BIT,
                    EGL14.EGL_RED_SIZE, 8, EGL14.EGL_GREEN_SIZE, 8, EGL14.EGL_BLUE_SIZE, 8,
                    EGL14.EGL_ALPHA_SIZE, 8, EGL14.EGL_NONE};
            require(EGL14.eglChooseConfig(display, attributes, 0, configs, 0, 1, size, 0) && size[0] > 0, "EGL config");
            eglContext = EGL14.eglCreateContext(display, configs[0], EGL14.EGL_NO_CONTEXT,
                    new int[] {EGL14.EGL_CONTEXT_CLIENT_VERSION, 3, EGL14.EGL_NONE}, 0);
            pbuffer = EGL14.eglCreatePbufferSurface(display, configs[0],
                    new int[] {EGL14.EGL_WIDTH, 128, EGL14.EGL_HEIGHT, 128, EGL14.EGL_NONE}, 0);
            require(EGL14.eglMakeCurrent(display, pbuffer, pbuffer, eglContext), "EGL context");
            String extensions = GLES30.glGetString(GLES30.GL_EXTENSIONS);
            require(extensions != null && extensions.contains("GL_EXT_YUV_target"), "GL_EXT_YUV_target");
            int program = GlesUpscaler.program(GlesUpscaler.shaderSource(context, "upscale.vert"),
                    GlesUpscaler.shaderSource(context, "packed444_probe.frag"));
            GLES30.glUseProgram(program);
            GLES30.glGenTextures(1, size, 0);
            GLES30.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, size[0]);
            GlesUpscaler.textureParameters(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES30.GL_NEAREST);
            texture = new SurfaceTexture(size[0]);
            texture.setDefaultBufferSize(128, 128);
            CountDownLatch available = new CountDownLatch(1);
            texture.setOnFrameAvailableListener(ignored -> available.countDown(), new Handler(callbacks.getLooper()));
            surface = new Surface(texture);
            mediaCodec = MediaCodec.createByCodecName(decoder);
            MediaFormat format = MediaFormat.createVideoFormat(codec.equals("hevc") ? "video/hevc" : "video/av01", 128, 128);
            sampleDataFormat(format);
            mediaCodec.configure(format, surface, null, 0);
            mediaCodec.start();
            ByteArrayOutputStream data = new ByteArrayOutputStream();
            try (InputStream input = context.getAssets().open("packed444/" + codec + "-" + (tenBit ? 10 : 8) + ".bin")) {
                byte[] buffer = new byte[4096];
                int count;
                while ((count = input.read(buffer)) >= 0) data.write(buffer, 0, count);
            }
            int input = mediaCodec.dequeueInputBuffer(1000000);
            require(input >= 0, "decoder input");
            mediaCodec.getInputBuffer(input).put(data.toByteArray());
            mediaCodec.queueInputBuffer(input, 0, data.size(), 1000, MediaCodec.BUFFER_FLAG_END_OF_STREAM);
            MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
            long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(3);
            boolean rendered = false;
            while (System.nanoTime() < deadline) {
                int output = mediaCodec.dequeueOutputBuffer(info, 10000);
                if (output >= 0) {
                    mediaCodec.releaseOutputBuffer(output, true);
                    rendered = true;
                    break;
                }
            }
            require(rendered && available.await(2, TimeUnit.SECONDS), "decoder output");
            texture.updateTexImage();
            float[] transform = new float[16];
            texture.getTransformMatrix(transform);
            GLES30.glUniformMatrix4fv(GLES30.glGetUniformLocation(program, "textureTransform"), 1, false, transform, 0);
            GLES30.glUniform1i(GLES30.glGetUniformLocation(program, "source"), 0);
            GLES30.glUniform1i(GLES30.glGetUniformLocation(program, "maximum"), tenBit ? 1023 : 255);
            GLES30.glDisable(GLES30.GL_DITHER);
            GLES30.glViewport(0, 0, 128, 128);
            GLES30.glDrawArrays(GLES30.GL_TRIANGLES, 0, 3);
            ByteBuffer pixels = ByteBuffer.allocateDirect(128 * 128 * 4);
            GLES30.glReadPixels(0, 0, 128, 128, GLES30.GL_RGBA, GLES30.GL_UNSIGNED_BYTE, pixels);
            require(GLES30.glGetError() == GLES30.GL_NO_ERROR, "raw YUV shader");
            for (int i = 0; i < pixels.capacity(); i++) require((pixels.get(i) & 255) == 255, "raw sample precision");
            return true;
        } catch (Exception e) {
            LimeLog.warning("Packed chroma " + decoder + " " + (tenBit ? 10 : 8) + "-bit probe failed: " + e);
            if (e instanceof InterruptedException) Thread.currentThread().interrupt();
            return false;
        } finally {
            if (mediaCodec != null) mediaCodec.release();
            if (surface != null) surface.release();
            if (texture != null) texture.release();
            EGL14.eglMakeCurrent(display, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT);
            if (pbuffer != EGL14.EGL_NO_SURFACE) EGL14.eglDestroySurface(display, pbuffer);
            if (eglContext != EGL14.EGL_NO_CONTEXT) EGL14.eglDestroyContext(display, eglContext);
            if (display != EGL14.EGL_NO_DISPLAY) EGL14.eglTerminate(display);
            EGL14.eglReleaseThread();
            callbacks.quitSafely();
        }
    }

    static void sampleDataFormat(MediaFormat format) {
        format.setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_FULL);
        format.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT709);
        format.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO);
    }

    private static void require(boolean value, String operation) {
        if (!value) throw new IllegalStateException(operation + " unavailable");
    }
}
