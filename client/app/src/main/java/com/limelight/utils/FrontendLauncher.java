package com.limelight.utils;

import com.limelight.R;

/** Launcher setup uses the installed flavor's package and each launcher's own URI placeholder. */
public enum FrontendLauncher {
    BEACON("Beacon", R.string.frontend_setup_beacon),
    ES_DE("ES-DE", R.string.frontend_setup_esde),
    DAIJISHO("Daijisho", R.string.frontend_setup_daijisho),
    PEGASUS("Pegasus", R.string.frontend_setup_pegasus);

    public final String label;
    public final int instructions;

    FrontendLauncher(String label, int instructions) {
        this.label = label;
        this.instructions = instructions;
    }

    public String command(String packageName) {
        if (this == ES_DE) return null; // The exporter merges ES-DE's configuration itself.
        String command = "-n " + packageName + "/com.limelight.ShortcutTrampoline" +
                " -a android.intent.action.VIEW --grant-read-uri-permission -d " +
                (this == BEACON ? "{file_uri}" : "{file.uri}");
        return this == DAIJISHO ? command : "am start " + command;
    }
}
