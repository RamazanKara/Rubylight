package com.limelight.update;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.annotation.Config;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/** org.json is only real under Robolectric, so the parsing runs there. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class ReleaseInfoTest {
    private static String asset(String name, long size) {
        return "{\"name\":\"" + name + "\",\"size\":" + size
                + ",\"browser_download_url\":\"https://github.com/RamazanKara/nightjar/releases/download/v0.7.0/" + name + "\"}";
    }

    private static String release(String tag, String body, boolean draft, boolean prerelease, String... assets) {
        return "{\"tag_name\":\"" + tag + "\",\"body\":" + body + ",\"draft\":" + draft + ",\"prerelease\":" + prerelease
                + ",\"assets\":[" + String.join(",", assets) + "],\"id\":1,\"url\":\"x\"}";
    }

    private static ReleaseInfo v070() throws Exception {
        return ReleaseInfo.parse(release("v0.7.0", "\"## Changes\\n- Faster\"", false, false,
                asset("nightjar-0.7.0-root.apk", 18008391), asset("nightjar-0.7.0.aab", 25129586),
                asset("nightjar-0.7.0.apk", 17987490), asset("SHA256SUMS-0.7.0.txt", 260)));
    }

    @Test
    public void reads_the_tag_notes_and_assets() throws Exception {
        ReleaseInfo release = v070();
        assertEquals("v0.7.0", release.tag);
        assertEquals("0.7.0", release.version());
        assertEquals("## Changes\n- Faster", release.body);
        assertFalse(release.draft);
        assertFalse(release.prerelease);
        assertEquals(4, release.assets.size());
        assertEquals(17987490, release.assets.get(2).size);
        assertTrue(release.assets.get(2).url.startsWith("https://github.com/"));
    }

    @Test
    public void picks_the_apk_for_the_flavor() throws Exception {
        ReleaseInfo release = v070();
        assertEquals("nightjar-0.7.0.apk", release.apkAsset(false).name);
        assertEquals("nightjar-0.7.0-root.apk", release.apkAsset(true).name);
        assertEquals("SHA256SUMS-0.7.0.txt", release.checksumAsset().name);
    }

    @Test
    public void a_release_without_the_flavor_or_the_sums_has_none() throws Exception {
        ReleaseInfo onlyAab = ReleaseInfo.parse(release("v0.8.0", "null", false, false, asset("nightjar-0.8.0.aab", 1)));
        assertNull(onlyAab.apkAsset(false));
        assertNull(onlyAab.apkAsset(true));
        assertNull(onlyAab.checksumAsset());
        ReleaseInfo noRoot = ReleaseInfo.parse(release("v0.8.0", "null", false, false, asset("nightjar-0.8.0.apk", 1)));
        assertNotNull(noRoot.apkAsset(false));
        assertNull(noRoot.apkAsset(true));
    }

    @Test
    public void null_notes_and_missing_fields_are_tolerated() throws Exception {
        ReleaseInfo release = ReleaseInfo.parse("{\"tag_name\":\"v1.0.0\",\"body\":null}");
        assertEquals("", release.body);
        assertTrue(release.assets.isEmpty());
        assertFalse(release.draft);
    }

    @Test(expected = org.json.JSONException.class)
    public void a_release_without_a_tag_is_rejected() throws Exception {
        ReleaseInfo.parse("{\"name\":\"x\"}");
    }

    @Test
    public void only_a_newer_final_release_is_an_update() throws Exception {
        assertTrue(v070().isUpdateFor("0.6.0"));
        assertTrue(v070().isUpdateFor("0.6.9-debug"));
        assertFalse(v070().isUpdateFor("0.7.0"));
        assertFalse(v070().isUpdateFor("0.7.1"));
        assertFalse(v070().isUpdateFor("not a version"));
        assertFalse(ReleaseInfo.parse(release("v0.8.0", "null", true, false)).isUpdateFor("0.7.0"));
        assertFalse(ReleaseInfo.parse(release("v0.8.0", "null", false, true)).isUpdateFor("0.7.0"));
        assertFalse(ReleaseInfo.parse(release("v0.8.0-rc.1", "null", false, false)).isUpdateFor("0.7.0"));
        assertFalse(ReleaseInfo.parse(release("nightly", "null", false, false)).isUpdateFor("0.7.0"));
    }

    @Test
    public void a_3_0_release_candidate_user_is_offered_only_the_final_3_0() throws Exception {
        assertFalse(ReleaseInfo.parse(release("v3.0.0-rc.1", "null", false, true)).isUpdateFor("0.8.0"));
        assertFalse(ReleaseInfo.parse(release("v0.8.1", "null", false, false)).isUpdateFor("3.0.0-rc.1"));
        assertTrue(ReleaseInfo.parse(release("v3.0.0", "null", false, false)).isUpdateFor("3.0.0-rc.1"));
        assertTrue(ReleaseInfo.parse(release("v3.0.0", "null", false, false)).isUpdateFor("0.8.0"));
    }
}
