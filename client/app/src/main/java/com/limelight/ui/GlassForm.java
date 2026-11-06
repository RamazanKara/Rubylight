package com.limelight.ui;

import android.content.Context;
import android.content.res.Configuration;

/**
 * Which of the design's three layouts a window gets: by the window's type and shape, never by the device's name.
 * Chosen in code because the width and orientation qualifiers outrank {@code -television} in resource matching.
 */
public enum GlassForm {
    PHONE, HANDHELD, TELEVISION;

    public static GlassForm of(Context context) {
        Configuration config = context.getResources().getConfiguration();
        if ((config.uiMode & Configuration.UI_MODE_TYPE_MASK) == Configuration.UI_MODE_TYPE_TELEVISION) return TELEVISION;
        return config.orientation == Configuration.ORIENTATION_LANDSCAPE && config.screenWidthDp >= 600 ? HANDHELD : PHONE;
    }
}
