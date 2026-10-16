package com.limelight.update;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;

/** The parts of a GitHub release that the updater uses. */
public final class ReleaseInfo {
    public static final class Asset {
        public final String name;
        public final String url;
        public final long size;

        public Asset(String name, String url, long size) {
            this.name = name;
            this.url = url;
            this.size = size;
        }
    }

    public final String tag;
    public final String body;
    public final boolean draft;
    public final boolean prerelease;
    public final List<Asset> assets;

    public ReleaseInfo(String tag, String body, boolean draft, boolean prerelease, List<Asset> assets) {
        this.tag = tag;
        this.body = body == null ? "" : body;
        this.draft = draft;
        this.prerelease = prerelease;
        this.assets = Collections.unmodifiableList(new ArrayList<>(assets));
    }

    public static ReleaseInfo parse(String json) throws JSONException {
        JSONObject release = new JSONObject(json);
        String tag = release.getString("tag_name");
        List<Asset> assets = new ArrayList<>();
        JSONArray array = release.optJSONArray("assets");
        for (int i = 0; array != null && i < array.length(); i++) {
            JSONObject asset = array.optJSONObject(i);
            if (asset == null || asset.isNull("name") || asset.isNull("browser_download_url")) {
                continue;
            }
            assets.add(new Asset(asset.getString("name"), asset.getString("browser_download_url"), asset.optLong("size", -1)));
        }
        return new ReleaseInfo(tag, release.isNull("body") ? "" : release.optString("body", ""),
                release.optBoolean("draft", false), release.optBoolean("prerelease", false), assets);
    }

    /** The version without a leading v. */
    public String version() {
        return SemVer.normalize(tag);
    }

    /** Whether this release is a final one newer than the running version. */
    public boolean isUpdateFor(String currentVersion) {
        SemVer release = SemVer.parse(tag);
        return !draft && !prerelease && release != null && !release.isPrerelease()
                && SemVer.isNewer(tag, currentVersion);
    }

    /** The APK for this build: nightjar-X.Y.Z.apk, or nightjar-X.Y.Z-root.apk for the root flavor. */
    public Asset apkAsset(boolean root) {
        Asset fallback = null;
        for (Asset asset : assets) {
            String name = asset.name.toLowerCase(Locale.ROOT);
            if (!name.endsWith(".apk") || name.endsWith("-root.apk") != root) {
                continue;
            }
            if (name.startsWith("nightjar-")) {
                return asset;
            }
            if (fallback == null) {
                fallback = asset;
            }
        }
        return fallback;
    }

    /** SHA256SUMS-X.Y.Z.txt, or null when the release has none. */
    public Asset checksumAsset() {
        Asset fallback = null;
        for (Asset asset : assets) {
            String name = asset.name.toLowerCase(Locale.ROOT);
            if (!name.startsWith("sha256sums") || !name.endsWith(".txt")) {
                continue;
            }
            if (name.equals("sha256sums-" + version().toLowerCase(Locale.ROOT) + ".txt")) {
                return asset;
            }
            if (fallback == null) {
                fallback = asset;
            }
        }
        return fallback;
    }
}
