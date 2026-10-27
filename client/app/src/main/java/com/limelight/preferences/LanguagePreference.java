package com.limelight.preferences;

import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Build;
import android.provider.Settings;
import android.util.AttributeSet;

public class LanguagePreference extends MaterialListPreference {
    public LanguagePreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    @Override
    protected void onClick() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            try {
                // Launch the Android native app locale settings page
                Intent intent = new Intent(Settings.ACTION_APP_LOCALE_SETTINGS);
                intent.addCategory(Intent.CATEGORY_DEFAULT);
                intent.setData(Uri.parse("package:" + getContext().getPackageName()));
                getContext().startActivity(intent, null);
                return;
            } catch (ActivityNotFoundException e) {
                // App locale settings should be present on all Android 13 devices,
                // but if not, we'll launch the old language chooser.
            }
        }

        // If we don't have native app locale settings, launch the normal dialog
        super.onClick();
    }
}
