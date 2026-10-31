package com.limelight.binding.audio;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.Context;
import android.media.AudioAttributes;
import android.media.AudioDeviceInfo;
import android.media.AudioFormat;
import android.media.AudioManager;
import android.media.AudioRecord;
import android.media.AudioTrack;
import android.media.audiofx.AudioEffect;
import android.os.Build;
import android.os.Bundle;
import android.util.Log;

import androidx.annotation.RequiresApi;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.lang.reflect.Method;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.Random;

/**
 * Debug only: how much of what this device plays reaches its own microphone, per playback path and
 * capture plan. Plays a known noise signal and records it at the same time; logs levels under the
 * tag EchoProbe and writes WAVs to the app's external files folder for offline analysis.
 *
 * adb shell am start -n PKG/com.limelight.binding.audio.EchoProbeActivity --es arms aaudio:mic,aaudio:vc,aaudio:own
 *
 * An arm is play:capture[:comm]. play: aaudio (the stream's native output), track (AudioTrack,
 * USAGE_GAME), voice (AudioTrack, USAGE_VOICE_COMMUNICATION), none. capture: mic (echo off), vc
 * (Android's canceller on the voice source), vcraw (voice source, effects off), own (Nightjar's
 * EchoCanceller on the plain source, fed with what the arm plays). comm sets MODE_IN_COMMUNICATION
 * on the speaker.
 *
 * --es engine own|platform|both stores the echo engine streams use (MicProcessing.EchoEngine) and
 * runs no arms unless they are given too.
 */
public class EchoProbeActivity extends Activity {
    private static final String TAG = "EchoProbe";
    private static final int RATE = 48000;
    private static final int LEAD_MS = 600, SIGNAL_MS = 5000, TAIL_MS = 600;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        String engine = getIntent().getStringExtra("engine");
        String arms = getIntent().getStringExtra("arms");
        if (engine != null) {
            MicProcessing.EchoEngine chosen = MicProcessing.EchoEngine.of(engine);
            android.preference.PreferenceManager.getDefaultSharedPreferences(this).edit()
                    .putString(com.limelight.preferences.PreferenceConfiguration.MIC_ECHO_ENGINE_PREF_STRING, chosen.key)
                    .commit();
            Log.i(TAG, "engine " + chosen.key);
            if (arms == null) {
                Log.i(TAG, "done");
                finish();
                return;
            }
        }
        if (arms == null) arms = "aaudio:mic,aaudio:vc,aaudio:own";
        float level = getIntent().getFloatExtra("level", 0.1f);
        String[] list = arms.split(",");
        new Thread(() -> {
            logEffects();
            for (String arm : list) {
                try {
                    runArm(arm.trim(), level);
                } catch (Exception e) {
                    Log.e(TAG, "arm " + arm + " failed", e);
                }
            }
            Log.i(TAG, "done");
            runOnUiThread(this::finish);
        }, "EchoProbe").start();
    }

    private void logEffects() {
        for (AudioEffect.Descriptor d : AudioEffect.queryEffects()) {
            if (d.type.equals(AudioEffect.EFFECT_TYPE_AEC) || d.type.equals(AudioEffect.EFFECT_TYPE_NS) ||
                    d.type.equals(AudioEffect.EFFECT_TYPE_AGC)) {
                Log.i(TAG, "effect " + d.name + " by " + d.implementor + " mode " + d.connectMode + " uuid " + d.uuid);
            }
        }
    }

    private static short[] signal(float level) {
        int lead = RATE * LEAD_MS / 1000, sig = RATE * SIGNAL_MS / 1000, tail = RATE * TAIL_MS / 1000;
        short[] mono = new short[lead + sig + tail];
        Random random = new Random(56);
        // White noise in bursts of 400 ms on, 100 ms off: an echo canceller sees a broadband, changing signal.
        for (int i = 0; i < sig; i++) {
            boolean on = (i % (RATE / 2)) < RATE * 4 / 10;
            float v = on ? (float) (random.nextGaussian() * level) : 0f;
            mono[lead + i] = (short) Math.max(-32767, Math.min(32767, Math.round(v * 32767)));
        }
        return mono;
    }

    @SuppressLint("MissingPermission")
    private void runArm(String arm, float level) throws Exception {
        String[] parts = arm.split(":");
        String play = parts[0], capture = parts.length > 1 ? parts[1] : "mic";
        boolean comm = parts.length > 2 && parts[2].equals("comm");
        AudioManager audio = (AudioManager) getSystemService(Context.AUDIO_SERVICE);
        short[] mono = signal(level);
        if (comm) setCommunication(audio, true);
        MicProcessing processing = capture.equals("mic") ? new MicProcessing(MicProcessing.Echo.OFF, false, false, 100)
                : new MicProcessing(MicProcessing.Echo.ON, true, false, 100, capture.equals("own") ?
                        MicProcessing.EchoEngine.OWN : MicProcessing.EchoEngine.PLATFORM);
        MicEffects effects = new MicEffects();
        MicProcessing.Plan plan = processing.plan(false, effects);
        EchoCanceller canceller = plan.ownEchoCanceller ? EchoCanceller.open() : null;
        if (plan.ownEchoCanceller && canceller == null) throw new IllegalStateException("the echo canceller did not open");
        int min = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT);
        AudioRecord record = new AudioRecord(plan.source, RATE, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT, Math.max(min, RATE / 5));
        int source = plan.source;
        if (capture.equals("vcraw")) {
            plan = new MicProcessing(MicProcessing.Echo.OFF, false, false, 100).plan(false, effects);
        }
        java.util.List<MicProcessing.Effect> failed = effects.attach(record.getAudioSessionId(), plan);
        short[] recorded = new short[mono.length + RATE];
        record.startRecording();
        final int[] recordedCount = {0};
        Thread reader = new Thread(() -> {
            // 20 ms frames, as MicrophoneCapture reads them.
            short[] frame = new short[MicrophoneCapture.FRAME_SAMPLES];
            int offset = 0;
            while (offset + frame.length <= recorded.length) {
                int got = 0;
                while (got < frame.length) {
                    int n = record.read(frame, got, frame.length - got);
                    if (n <= 0) return;
                    got += n;
                }
                if (canceller != null && !canceller.process(frame, frame.length)) {
                    Log.e(TAG, "the echo canceller refused a frame");
                    return;
                }
                System.arraycopy(frame, 0, recorded, offset, frame.length);
                offset += frame.length;
                synchronized (recordedCount) {
                    recordedCount[0] = offset;
                }
            }
        });
        reader.start();
        Thread.sleep(200);
        int startAt;
        synchronized (recordedCount) {
            startAt = recordedCount[0];
        }
        String route = routeOf(record);
        play(play, mono, audio);
        reader.join(10000);
        record.stop();
        record.release();
        effects.release();
        String aec = "";
        if (canceller != null) {
            aec = ", " + canceller.stats();
            canceller.close();
        }
        if (comm) setCommunication(audio, false);

        // Levels: ambient before the signal reaches the speaker, echo while it plays (after 1 s to let
        // an echo canceller converge, before the end with room for output latency).
        int lead = RATE * LEAD_MS / 1000;
        double ambient = rms(recorded, startAt + RATE / 20, startAt + lead - RATE / 20);
        double echo = rms(recorded, startAt + lead + RATE, startAt + lead + RATE * (SIGNAL_MS - 500) / 1000);
        Log.i(TAG, String.format(java.util.Locale.ROOT,
                "arm %s source %d aec %b own %b ns %b failed %s route %s mode %d: ambient %.1f dBFS, echo %.1f dBFS, start %d%s",
                arm, source, plan.echoCanceler, plan.ownEchoCanceller, plan.noiseSuppressor, failed, route,
                audio.getMode(), db(ambient), db(echo), startAt, aec));
        File dir = new File(getExternalFilesDir(null), "echo");
        //noinspection ResultOfMethodCallIgnored
        dir.mkdirs();
        writeWav(new File(dir, arm.replace(':', '_') + ".wav"), recorded);
        writeWav(new File(dir, "reference.wav"), mono);
        try (FileOutputStream out = new FileOutputStream(new File(dir, arm.replace(':', '_') + ".start"))) {
            out.write(Integer.toString(startAt).getBytes());
        }
    }

    private static String routeOf(AudioRecord record) {
        if (Build.VERSION.SDK_INT < 24) return "?";
        AudioDeviceInfo device = record.getRoutedDevice();
        return device == null ? "none" : device.getType() + "/" + device.getProductName();
    }

    private void setCommunication(AudioManager audio, boolean on) {
        if (on) {
            audio.setMode(AudioManager.MODE_IN_COMMUNICATION);
            if (Build.VERSION.SDK_INT >= 31) {
                for (AudioDeviceInfo device : audio.getAvailableCommunicationDevices()) {
                    if (device.getType() == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER) {
                        Log.i(TAG, "communication device speaker: " + audio.setCommunicationDevice(device));
                    }
                }
            } else {
                audio.setSpeakerphoneOn(true);
            }
        } else {
            if (Build.VERSION.SDK_INT >= 31) audio.clearCommunicationDevice();
            else audio.setSpeakerphoneOn(false);
            audio.setMode(AudioManager.MODE_NORMAL);
        }
    }

    private void play(String play, short[] mono, AudioManager audio) throws Exception {
        short[] stereo = new short[mono.length * 2];
        for (int i = 0; i < mono.length; i++) {
            stereo[2 * i] = mono[i];
            stereo[2 * i + 1] = mono[i];
        }
        int chunk = RATE / 100 * 2;
        switch (play) {
            case "none":
                Thread.sleep(mono.length * 1000L / RATE);
                return;
            case "aaudio": {
                Class.forName("com.limelight.nvstream.jni.MoonBridge");
                Method open = AndroidAudioRenderer.class.getDeclaredMethod("nativeOpenAudio", int.class, long.class);
                Method write = AndroidAudioRenderer.class.getDeclaredMethod("nativeWriteAudio", long.class, short[].class, int.class);
                Method queued = AndroidAudioRenderer.class.getDeclaredMethod("nativeAudioQueued", long.class);
                Method close = AndroidAudioRenderer.class.getDeclaredMethod("nativeCloseAudio", long.class);
                open.setAccessible(true);
                write.setAccessible(true);
                queued.setAccessible(true);
                close.setAccessible(true);
                long handle = (long) open.invoke(null, RATE, 0L);
                if (handle == 0) throw new IllegalStateException("AAudio did not open");
                short[] part = new short[chunk];
                for (int offset = 0; offset < stereo.length; offset += chunk) {
                    int ahead;
                    while ((ahead = (int) queued.invoke(null, handle)) > RATE / 25) Thread.sleep(2);
                    int n = Math.min(chunk, stereo.length - offset);
                    System.arraycopy(stereo, offset, part, 0, n);
                    write.invoke(null, handle, part, n);
                    // As AndroidAudioRenderer feeds it after each write.
                    EchoCanceller.reference(part, n, 2, ahead * 1000 / RATE);
                }
                Thread.sleep(200);
                close.invoke(null, handle);
                return;
            }
            default:
                // The AudioTrack arms use AudioTrack.Builder; minSdk is 21.
                if (Build.VERSION.SDK_INT < 23) throw new IllegalStateException("the " + play + " arm needs Android 6 or newer");
                playTrack(play.equals("voice"), stereo, chunk);
        }
    }

    @RequiresApi(23)
    private static void playTrack(boolean voice, short[] stereo, int chunk) throws InterruptedException {
        AudioAttributes.Builder attributes = new AudioAttributes.Builder()
                .setUsage(voice ? AudioAttributes.USAGE_VOICE_COMMUNICATION : AudioAttributes.USAGE_GAME);
        if (voice) attributes.setContentType(AudioAttributes.CONTENT_TYPE_SPEECH);
        AudioTrack.Builder builder = new AudioTrack.Builder()
                .setAudioAttributes(attributes.build())
                .setAudioFormat(new AudioFormat.Builder().setSampleRate(RATE)
                        .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                        .setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
                .setBufferSizeInBytes(Math.max(AudioTrack.getMinBufferSize(RATE,
                        AudioFormat.CHANNEL_OUT_STEREO, AudioFormat.ENCODING_PCM_16BIT), chunk * 2 * 4))
                .setTransferMode(AudioTrack.MODE_STREAM);
        if (Build.VERSION.SDK_INT >= 26) builder.setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY);
        AudioTrack track = builder.build();
        track.play();
        for (int offset = 0; offset < stereo.length; offset += chunk) {
            int n = Math.min(chunk, stereo.length - offset);
            track.write(stereo, offset, n);
            short[] part = java.util.Arrays.copyOfRange(stereo, offset, offset + n);
            EchoCanceller.reference(part, n, 2, 0);
        }
        Thread.sleep(300);
        AudioDeviceInfo routed = track.getRoutedDevice();
        Log.i(TAG, "track routed to " + (routed == null ? "none" : routed.getType()));
        track.stop();
        track.release();
    }

    private static double rms(short[] pcm, int from, int to) {
        from = Math.max(0, from);
        to = Math.min(pcm.length, to);
        if (to <= from) return 0;
        double sum = 0;
        for (int i = from; i < to; i++) sum += (double) pcm[i] * pcm[i];
        return Math.sqrt(sum / (to - from)) / 32768.0;
    }

    private static double db(double value) {
        return 20 * Math.log10(Math.max(value, 1e-9));
    }

    private static void writeWav(File file, short[] pcm) throws IOException {
        ByteBuffer buffer = ByteBuffer.allocate(44 + pcm.length * 2).order(ByteOrder.LITTLE_ENDIAN);
        buffer.put("RIFF".getBytes()).putInt(36 + pcm.length * 2).put("WAVE".getBytes());
        buffer.put("fmt ".getBytes()).putInt(16).putShort((short) 1).putShort((short) 1).putInt(RATE)
                .putInt(RATE * 2).putShort((short) 2).putShort((short) 16);
        buffer.put("data".getBytes()).putInt(pcm.length * 2);
        for (short s : pcm) buffer.putShort(s);
        try (FileOutputStream out = new FileOutputStream(file)) {
            out.write(buffer.array());
        }
    }
}
