package com.limelight.preferences;

import android.Manifest;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Bundle;
import android.widget.TextView;

import androidx.camera.core.CameraSelector;
import androidx.camera.core.ImageAnalysis;
import androidx.camera.core.ImageProxy;
import androidx.camera.core.Preview;
import androidx.camera.lifecycle.ProcessCameraProvider;
import androidx.camera.view.PreviewView;
import androidx.core.content.ContextCompat;

import com.google.common.util.concurrent.ListenableFuture;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nvstream.http.PairingLink;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

import java.nio.ByteBuffer;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Scans the PC's pairing QR code (Experimental) and returns its link in {@link #EXTRA_LINK}. Add PC asks for the
 * camera before it opens this screen; other QR codes only change the hint.
 */
public class QrScanActivity extends NightjarActivity {
    public static final String EXTRA_LINK = "com.limelight.preferences.QrScanActivity.link";

    private final ExecutorService analysis = Executors.newSingleThreadExecutor(
            task -> new Thread(task, "QR scan"));
    private TextView status;
    private PreviewView preview;
    private volatile boolean found;
    // The analysis thread's copy of the frame, reused while the size stays.
    private byte[] frame;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        UiHelper.setLocale(this);
        setContentView(R.layout.activity_qr_scan);
        TopAppBar.setNavigation(TopAppBar.configure(this, R.id.qr_scan_toolbar, getString(R.string.qr_scan_title), 0),
                R.drawable.ic_close, android.R.string.cancel, R.id.qr_scan_back, v -> finish());
        UiHelper.notifyNewRootView(this);
        status = findViewById(R.id.qr_scan_status);
        preview = findViewById(R.id.qr_scan_preview);

        if (ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            cameraFailed(null);
            return;
        }
        ListenableFuture<ProcessCameraProvider> provider = ProcessCameraProvider.getInstance(this);
        provider.addListener(() -> {
            if (isFinishing() || isDestroyed()) return;
            try {
                bind(provider.get());
            } catch (Exception e) {
                // ExecutionException from the provider, CameraInfoUnavailableException, or the
                // IllegalArgumentException and IllegalStateException that binding throws.
                cameraFailed(e);
            }
        }, ContextCompat.getMainExecutor(this));
    }

    private void bind(ProcessCameraProvider provider) throws Exception {
        CameraSelector camera = provider.hasCamera(CameraSelector.DEFAULT_BACK_CAMERA)
                ? CameraSelector.DEFAULT_BACK_CAMERA : CameraSelector.DEFAULT_FRONT_CAMERA;
        Preview shown = new Preview.Builder().build();
        shown.setSurfaceProvider(preview.getSurfaceProvider());
        ImageAnalysis frames = new ImageAnalysis.Builder()
                .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                .build();
        frames.setAnalyzer(analysis, this::analyze);
        provider.unbindAll();
        provider.bindToLifecycle(this, camera, shown, frames);
    }

    private void analyze(ImageProxy image) {
        try {
            if (found) return;
            ImageProxy.PlaneProxy plane = image.getPlanes()[0];
            ByteBuffer buffer = plane.getBuffer();
            if (frame == null || frame.length != buffer.remaining()) frame = new byte[buffer.remaining()];
            buffer.get(frame);
            String text = QrCodes.decode(frame, image.getWidth(), image.getHeight(), plane.getRowStride());
            if (text != null) runOnUiThread(() -> read(text));
        } finally {
            image.close();
        }
    }

    private void read(String text) {
        if (found || isFinishing() || isDestroyed()) return;
        if (PairingLink.parse(text) != null) {
            found = true;
            setResult(RESULT_OK, new Intent().putExtra(EXTRA_LINK, text));
            finish();
        } else {
            status.setText(PairingLink.looksLikeLink(text) ? R.string.addpc_qr_invalid : R.string.qr_scan_not_pairing);
        }
    }

    private void cameraFailed(Exception e) {
        LimeLog.warning("QR scan: the camera did not start" + (e == null ? " (no permission)" : ": " + e));
        status.setText(R.string.qr_scan_camera_failed);
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        analysis.shutdownNow();
    }
}
