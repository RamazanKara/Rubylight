package com.limelight.preferences;

import android.app.Application;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.view.View;
import android.widget.LinearLayout;
import android.widget.RadioGroup;
import android.widget.TextView;
import android.widget.Toast;

import com.limelight.ui.NightjarActivity;
import androidx.lifecycle.AndroidViewModel;
import androidx.lifecycle.MutableLiveData;
import androidx.lifecycle.ViewModelProvider;

import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.ui.TopAppBar;
import com.google.android.material.button.MaterialButton;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.google.android.material.radiobutton.MaterialRadioButton;
import com.limelight.R;
import com.limelight.binding.video.GpuDriverStore;
import com.limelight.binding.video.PyroWaveDecoderRenderer;
import com.limelight.utils.UiHelper;

import java.io.IOException;
import java.io.InputStream;
import java.util.List;
import java.util.concurrent.Callable;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class GpuDriverSettings extends NightjarActivity {
    private static final int IMPORT_DRIVER = 1;
    private DriverWork work;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        UiHelper.setLocale(this);
        setContentView(R.layout.activity_gpu_driver_settings);
        MaterialToolbar toolbar = TopAppBar.configure(this, R.id.gpu_driver_toolbar, getString(R.string.gpu_driver_title), 0);
        TopAppBar.setUpNavigation(toolbar, R.id.gpu_driver_back, view -> finish());
        work = new ViewModelProvider(this).get(DriverWork.class);
        work.busy.observe(this, ignored -> refresh());
        work.message.observe(this, ignored -> refresh());
        refresh();
        UiHelper.notifyNewRootView(this);
        View root = findViewById(android.R.id.content);
        View content = findViewById(R.id.gpu_driver_root);
        root.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> {
            if (content.getLeft() != view.getPaddingLeft() || content.getTop() != view.getPaddingTop() ||
                    content.getRight() != view.getWidth() - view.getPaddingRight() ||
                    content.getBottom() != view.getHeight() - view.getPaddingBottom()) {
                // AdaptiveLayout can change padding during layout, after the children were positioned.
                content.post(content::requestLayout);
            }
        });
    }

    private void refresh() {
        LinearLayout content = findViewById(R.id.gpu_driver_content);
        content.removeAllViews();
        boolean busy = Boolean.TRUE.equals(work.busy.getValue());
        boolean supported = supportsDrivers();
        addText(content, getString(supported ? R.string.gpu_driver_explanation : R.string.gpu_driver_unsupported));
        GpuDriverStore.Driver selected = GpuDriverStore.selected(this);
        List<GpuDriverStore.Driver> drivers = GpuDriverStore.list(this);
        RadioGroup choices = new RadioGroup(this);
        content.addView(choices);
        addChoice(choices, getString(R.string.gpu_driver_system), null, selected == null, !busy);
        for (GpuDriverStore.Driver driver : drivers) {
            addChoice(choices, driver.name, driver, selected != null && selected.id.equals(driver.id), supported && !busy);
        }
        choices.setOnCheckedChangeListener((group, id) -> {
            GpuDriverStore.Driver driver = (GpuDriverStore.Driver) group.findViewById(id).getTag();
            Context app = getApplicationContext();
            work.run(() -> {
                GpuDriverStore.select(app, driver);
                return app.getString(PyroWaveDecoderRenderer.getReadinessSummary(app, false));
            });
        });
        if (selected != null && !selected.description.isEmpty()) addText(content, selected.description);
        if (GpuDriverStore.usedFallback(this)) addText(content, getString(R.string.gpu_driver_system_fallback));
        if (busy) addText(content, getString(R.string.gpu_driver_working));
        else if (work.message.getValue() != null) addText(content, work.message.getValue());

        MaterialButton importButton = button(content, R.string.gpu_driver_import, com.google.android.material.R.attr.materialButtonStyle);
        importButton.setEnabled(supported && !busy);
        importButton.setOnClickListener(view -> {
            try {
                startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT)
                        .addCategory(Intent.CATEGORY_OPENABLE).setType("application/zip")
                        .putExtra(Intent.EXTRA_MIME_TYPES, new String[] {"application/zip", "application/x-zip-compressed", "application/octet-stream"}), IMPORT_DRIVER);
            } catch (ActivityNotFoundException e) {
                Toast.makeText(this, R.string.gpu_driver_no_picker, Toast.LENGTH_LONG).show();
            }
        });
        MaterialButton removeButton = button(content, R.string.gpu_driver_remove, R.attr.nightjarButtonDestructiveStyle);
        removeButton.setEnabled(!drivers.isEmpty() && !busy);
        removeButton.setOnClickListener(view -> new MaterialAlertDialogBuilder(this)
                .setTitle(R.string.gpu_driver_remove)
                .setItems(drivers.stream().map(driver -> driver.name).toArray(String[]::new), (dialog, index) -> {
                    Context app = getApplicationContext();
                    work.run(() -> {
                        GpuDriverStore.remove(app, drivers.get(index));
                        return app.getString(R.string.gpu_driver_removed);
                    });
                }).setNegativeButton(android.R.string.cancel, null).show());
    }

    /** Whether custom drivers can load here. The debug build's demo mode shows the screen as on a supported phone. */
    protected boolean supportsDrivers() {
        return GpuDriverStore.isSupported();
    }

    private void addChoice(RadioGroup group, String label, GpuDriverStore.Driver driver, boolean checked, boolean enabled) {
        MaterialRadioButton radio = new MaterialRadioButton(this);
        radio.setId(View.generateViewId());
        radio.setText(label);
        radio.setTag(driver);
        radio.setEnabled(enabled);
        group.addView(radio);
        if (checked) group.check(radio.getId());
    }

    private void addText(LinearLayout content, String text) {
        TextView view = new TextView(this);
        view.setTextAppearance(this, R.style.TextAppearance_Nightjar_Body);
        view.setText(text);
        int padding = Math.round(12 * getResources().getDisplayMetrics().density);
        view.setPadding(0, padding, 0, padding);
        content.addView(view);
    }

    private MaterialButton button(LinearLayout content, int title, int styleAttr) {
        MaterialButton button = new MaterialButton(this, null, styleAttr);
        button.setText(title);
        content.addView(button);
        return button;
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != IMPORT_DRIVER || resultCode != RESULT_OK || data == null || data.getData() == null) return;
        Uri uri = data.getData();
        Context app = getApplicationContext();
        work.run(() -> {
            try (InputStream input = app.getContentResolver().openInputStream(uri)) {
                if (input == null) throw new IOException("Cannot open the driver ZIP");
                GpuDriverStore.Driver driver = GpuDriverStore.importDriver(app, input);
                GpuDriverStore.select(app, driver);
                return app.getString(R.string.gpu_driver_imported, driver.name) + "\n" +
                        app.getString(PyroWaveDecoderRenderer.getReadinessSummary(app, false));
            }
        });
    }

    public static final class DriverWork extends AndroidViewModel {
        final MutableLiveData<Boolean> busy = new MutableLiveData<>(false);
        final MutableLiveData<String> message = new MutableLiveData<>();
        private final ExecutorService executor = Executors.newSingleThreadExecutor();

        public DriverWork(Application application) {
            super(application);
        }

        void run(Callable<String> operation) {
            if (Boolean.TRUE.equals(busy.getValue())) return;
            busy.setValue(true);
            executor.execute(() -> {
                try {
                    message.postValue(operation.call());
                } catch (Exception e) {
                    message.postValue(getApplication().getString(R.string.gpu_driver_error, e.getMessage()));
                } finally {
                    busy.postValue(false);
                }
            });
        }

        @Override
        protected void onCleared() {
            executor.shutdown();
        }
    }
}
