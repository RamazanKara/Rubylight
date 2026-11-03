package com.limelight.mic;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.media.AudioManager;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.PowerManager;
import android.widget.Toast;

import androidx.core.app.NotificationCompat;
import androidx.core.app.ServiceCompat;
import androidx.core.content.ContextCompat;

import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.PcView;
import com.limelight.R;
import com.limelight.binding.PlatformBinding;
import com.limelight.binding.audio.SilentAudioRenderer;
import com.limelight.binding.video.DiscardingDecoderRenderer;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.NvConnectionListener;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;

import java.security.cert.CertificateEncodingException;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;

/**
 * Microphone only: this device's microphone on the PC, with no picture or sound, while the screen is off or another
 * app is in front. A foreground service of type microphone with an ongoing notification and its Stop. The session is an
 * input-only one that sends nothing but the microphone ({@link NvApp#micOnly}); a call on this device pauses it until
 * the call ends. One session at a time: starting one for another PC ends the first, and starting a stream ends it
 * ({@link #stop}), since the device has a single connection to a PC.
 */
public class MicOnlyService extends Service {
    private static final String ACTION_START = BuildConfig.APPLICATION_ID + ".mic.START";
    private static final String ACTION_STOP = BuildConfig.APPLICATION_ID + ".mic.STOP";
    private static final String EXTRA_HOST = "host";
    private static final String EXTRA_PORT = "port";
    private static final String EXTRA_HTTPS_PORT = "httpsPort";
    private static final String EXTRA_UNIQUE_ID = "uniqueId";
    private static final String EXTRA_PC_NAME = "pcName";
    private static final String EXTRA_SERVER_CERT = "serverCert";
    private static final String CHANNEL = "mic_only";
    private static final int NOTIFICATION = 0x4d49;
    private static final int ENDED_NOTIFICATION = 0x4d4a;
    // Below Android 12, which has no mode listener: how often the call state is looked at.
    private static final long CALL_POLL_MS = 1000;

    // Whether a session runs in this process; a stream start asks it to end only then.
    private static volatile boolean running;

    private final Handler main = new Handler(Looper.getMainLooper());
    private MicOnlySession session;
    private NvConnection connection;
    private String pcName = "";
    private AudioManager audio;
    private AudioManager.OnModeChangedListener modeListener;
    private final Runnable callPoll = new Runnable() {
        @Override
        public void run() {
            modeChanged(audio.getMode());
            main.postDelayed(this, CALL_POLL_MS);
        }
    };
    private WifiManager.WifiLock wifiLock;
    private PowerManager.WakeLock wakeLock;
    // What the connection said last, shown when it fails.
    private volatile String lastMessage;

    /** The intent that starts a session with {@code computer}, or null when the PC has no address to reach. */
    public static Intent startIntent(Context context, ComputerDetails computer, String uniqueId) {
        if (computer.activeAddress == null) return null;
        Intent intent = new Intent(context, MicOnlyService.class).setAction(ACTION_START);
        intent.putExtra(EXTRA_HOST, computer.activeAddress.address);
        intent.putExtra(EXTRA_PORT, computer.activeAddress.port);
        intent.putExtra(EXTRA_HTTPS_PORT, computer.httpsPort);
        intent.putExtra(EXTRA_UNIQUE_ID, uniqueId);
        intent.putExtra(EXTRA_PC_NAME, computer.displayName());
        try {
            if (computer.serverCert != null) intent.putExtra(EXTRA_SERVER_CERT, computer.serverCert.getEncoded());
        } catch (CertificateEncodingException e) {
            LimeLog.warning("Microphone only: the PC's certificate could not be encoded: " + e);
        }
        return intent;
    }

    /** Ends a running session, if there is one: a stream is about to start. */
    public static void stop(Context context) {
        if (!running) return;
        context.startService(new Intent(context, MicOnlyService.class).setAction(ACTION_STOP));
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        // A restart by the system has no session to go back to.
        if (intent == null || !ACTION_START.equals(intent.getAction())) {
            end(null);
            return START_NOT_STICKY;
        }
        if (connection != null) end(null);
        pcName = intent.getStringExtra(EXTRA_PC_NAME);
        if (pcName == null) pcName = "";
        session = new MicOnlySession();
        try {
            ServiceCompat.startForeground(this, NOTIFICATION, notification(session.phase()),
                    Build.VERSION.SDK_INT >= Build.VERSION_CODES.R ? ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE : 0);
        } catch (RuntimeException e) {
            // Android 12 and later refuse it from the background, Android 14 without the microphone permission.
            LimeLog.warning("Microphone only: the foreground service was refused: " + e);
            Toast.makeText(this, getString(R.string.mic_only_failed, pcName), Toast.LENGTH_LONG).show();
            session.end();
            stopSelf();
            return START_NOT_STICKY;
        }
        running = true;
        X509Certificate cert = null;
        byte[] der = intent.getByteArrayExtra(EXTRA_SERVER_CERT);
        if (der != null) {
            try {
                cert = (X509Certificate) CertificateFactory.getInstance("X.509")
                        .generateCertificate(new java.io.ByteArrayInputStream(der));
            } catch (CertificateException e) {
                LimeLog.warning("Microphone only: the PC's certificate could not be read: " + e);
                end(getString(R.string.mic_only_failed, pcName));
                return START_NOT_STICKY;
            }
        }
        StreamConfiguration config = new StreamConfiguration.Builder()
                .setApp(NvApp.micOnly(getString(R.string.mic_only_action)))
                // The host negotiates a stream it never sends; these are the input-only defaults.
                .setResolution(1280, 720)
                .setLaunchRefreshRate(30)
                .setRefreshRate(30)
                .setBitrate(500)
                .enableLocalAudioPlayback(true)
                .setMaxPacketSize(1392)
                .setRemoteConfiguration(StreamConfiguration.STREAM_CFG_AUTO)
                .setSupportedVideoFormats(MoonBridge.VIDEO_FORMAT_H264)
                .setAudioConfiguration(MoonBridge.AUDIO_CONFIGURATION_STEREO)
                .build();
        connection = new NvConnection(getApplicationContext(),
                new ComputerDetails.AddressTuple(intent.getStringExtra(EXTRA_HOST), intent.getIntExtra(EXTRA_PORT, 0)),
                intent.getIntExtra(EXTRA_HTTPS_PORT, 0), intent.getStringExtra(EXTRA_UNIQUE_ID), config,
                PlatformBinding.getCryptoProvider(this), cert);
        acquireLocks();
        watchCalls();
        LimeLog.info("Microphone only: connecting to " + pcName);
        connection.start(new SilentAudioRenderer(), new DiscardingDecoderRenderer(), new Listener(connection));
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        end(null);
        super.onDestroy();
    }

    /** Ends the session: the connection closes and the service goes; {@code reason} non-null stays as a notification. */
    private void end(String reason) {
        main.removeCallbacks(callPoll);
        if (modeListener != null && audio != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            audio.removeOnModeChangedListener(modeListener);
        }
        modeListener = null;
        releaseLocks();
        NvConnection closing = connection;
        connection = null;
        if (session != null) session.end();
        running = false;
        if (closing != null) {
            LimeLog.info("Microphone only: ending" + (reason != null ? " (" + reason + ")" : ""));
            // Stopping waits for the native connection to close.
            new Thread(closing::stop, "Microphone only stop").start();
        }
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE);
        if (reason != null) {
            notifications().notify(ENDED_NOTIFICATION, builder()
                    .setContentTitle(getString(R.string.mic_only_stopped))
                    .setContentText(reason)
                    .setAutoCancel(true)
                    .build());
        }
        stopSelf();
    }

    private void watchCalls() {
        audio = (AudioManager) getSystemService(Context.AUDIO_SERVICE);
        if (audio == null) return;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            modeListener = this::modeChanged;
            audio.addOnModeChangedListener(ContextCompat.getMainExecutor(this), modeListener);
            modeChanged(audio.getMode());
        } else {
            main.post(callPoll);
        }
    }

    // On the main thread.
    private void modeChanged(int mode) {
        if (session == null || !session.call(MicOnlySession.isCall(mode))) return;
        LimeLog.info("Microphone only: " + (session.muted() ? "paused for a call" : "the call ended, sending again"));
        applyMute();
        showPhase();
    }

    private void applyMute() {
        if (connection != null && session.phase() != MicOnlySession.Phase.CONNECTING) {
            connection.setMicrophoneMuted(session.muted());
        }
    }

    private void showPhase() {
        if (session.phase() == MicOnlySession.Phase.ENDED) return;
        notifications().notify(NOTIFICATION, notification(session.phase()));
    }

    private NotificationManager notifications() {
        NotificationManager manager = (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && manager.getNotificationChannel(CHANNEL) == null) {
            manager.createNotificationChannel(new NotificationChannel(CHANNEL, getString(R.string.mic_only_action),
                    NotificationManager.IMPORTANCE_LOW));
        }
        return manager;
    }

    private NotificationCompat.Builder builder() {
        notifications();
        Intent open = new Intent(this, PcView.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        return new NotificationCompat.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.ic_mic)
                .setContentIntent(PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_IMMUTABLE))
                .setCategory(NotificationCompat.CATEGORY_SERVICE);
    }

    private Notification notification(MicOnlySession.Phase phase) {
        int title;
        switch (phase) {
            case LIVE:
                title = R.string.mic_only_on;
                break;
            case PAUSED_FOR_CALL:
                title = R.string.mic_only_paused_call;
                break;
            default:
                title = R.string.mic_only_connecting;
                break;
        }
        PendingIntent stop = PendingIntent.getService(this, 1,
                new Intent(this, MicOnlyService.class).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE);
        return builder()
                .setContentTitle(getString(title))
                .setContentText(pcName)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
                .addAction(R.drawable.ic_stop, getString(R.string.mic_only_stop), stop)
                .build();
    }

    // Wi-Fi at full power and the CPU awake while the screen is off, as a stream keeps them.
    private void acquireLocks() {
        try {
            WifiManager wifi = (WifiManager) getApplicationContext().getSystemService(Context.WIFI_SERVICE);
            if (wifi != null) {
                wifiLock = wifi.createWifiLock(Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q
                        ? WifiManager.WIFI_MODE_FULL_LOW_LATENCY : WifiManager.WIFI_MODE_FULL_HIGH_PERF,
                        BuildConfig.BRAND_NAME + " Microphone only");
                wifiLock.setReferenceCounted(false);
                wifiLock.acquire();
            }
            PowerManager power = (PowerManager) getSystemService(Context.POWER_SERVICE);
            if (power != null) {
                wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, BuildConfig.BRAND_NAME + ":MicrophoneOnly");
                wakeLock.setReferenceCounted(false);
                wakeLock.acquire();
            }
        } catch (SecurityException e) {
            // Some Samsung devices refuse WifiLock.acquire() despite WAKE_LOCK, as for streams.
            LimeLog.warning("Microphone only: a lock was refused: " + e);
        }
    }

    private void releaseLocks() {
        if (wifiLock != null) wifiLock.release();
        if (wakeLock != null) wakeLock.release();
        wifiLock = null;
        wakeLock = null;
    }

    /** The connection's events, on its own threads; each goes to the main thread for the session it belongs to. */
    private final class Listener implements NvConnectionListener {
        private final NvConnection owner;

        Listener(NvConnection owner) {
            this.owner = owner;
        }

        private void onMain(Runnable action) {
            main.post(() -> {
                if (connection == owner) action.run();
            });
        }

        @Override
        public void stageStarting(String stage) {
        }

        @Override
        public void stageComplete(String stage) {
        }

        @Override
        public void stageFailed(String stage, int portFlags, int errorCode) {
            LimeLog.warning("Microphone only: " + stage + " failed (" + errorCode + ")");
            onMain(() -> end(lastMessage != null ? lastMessage : getString(R.string.mic_only_failed, pcName)));
        }

        @Override
        public void connectionStarted() {
            onMain(() -> {
                if (!session.connected()) return;
                LimeLog.info("Microphone only: sending to " + pcName);
                applyMute();
                showPhase();
            });
        }

        @Override
        public void connectionTerminated(int errorCode) {
            LimeLog.info("Microphone only: the connection ended (" + errorCode + ")");
            onMain(() -> end(errorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION ? null
                    : getString(R.string.mic_only_failed, pcName)));
        }

        @Override
        public void connectionStatusUpdate(int connectionStatus) {
        }

        @Override
        public void displayMessage(String message) {
            lastMessage = message;
        }

        @Override
        public void displayTransientMessage(String message) {
        }

        @Override
        public void rumble(short controllerNumber, short lowFreqMotor, short highFreqMotor) {
        }

        @Override
        public void rumbleTriggers(short controllerNumber, short leftTrigger, short rightTrigger) {
        }

        @Override
        public void setHdrMode(boolean enabled, byte[] hdrMetadata) {
        }

        @Override
        public void setMotionEventState(short controllerNumber, byte motionType, short reportRateHz) {
        }

        @Override
        public void setControllerLED(short controllerNumber, byte r, byte g, byte b) {
        }
    }
}
