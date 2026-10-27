package com.limelight.update;

/** When to look for an update, and whether to tell the user about what was found. */
public final class UpdatePolicy {
    public static final long CHECK_INTERVAL_MS = 12L * 60 * 60 * 1000;

    /** Stores that update the app themselves; the in-app updater stays out of their way. */
    private static final String PLAY_STORE = "com.android.vending";

    private UpdatePolicy() {
    }

    /** An automatic check runs when it is on and none ran in the last 12 hours. A clock set back counts as due. */
    public static boolean isCheckDue(boolean autoCheck, long lastCheckMs, long nowMs) {
        if (!autoCheck) {
            return false;
        }
        return lastCheckMs <= 0 || nowMs < lastCheckMs || nowMs - lastCheckMs >= CHECK_INTERVAL_MS;
    }

    /** Whether an automatic check should show the release: a final release newer than this build, not skipped. */
    public static boolean shouldOffer(ReleaseInfo release, String currentVersion, String skippedVersion) {
        if (release == null || !release.isUpdateFor(currentVersion)) {
            return false;
        }
        return skippedVersion == null || skippedVersion.isEmpty()
                || !SemVer.normalize(skippedVersion).equals(release.version());
    }

    /** Google Play updates the app it installed. */
    public static boolean isManagedByStore(String installerPackage) {
        return PLAY_STORE.equals(installerPackage);
    }

    /** Whether this build updates itself: not installed from a store and not built for one. */
    public static boolean isSelfUpdateAvailable(String installerPackage, boolean githubDistribution) {
        return githubDistribution && !isManagedByStore(installerPackage);
    }
}
