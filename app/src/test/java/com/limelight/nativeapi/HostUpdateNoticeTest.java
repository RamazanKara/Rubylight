package com.limelight.nativeapi;

import android.app.Application;
import android.content.Context;

import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.update.HostUpdateChecker;

import org.json.JSONObject;
import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.robolectric.Shadows.shadowOf;

/** The PC tile's "Rubylight update" badge: the host's own answer first, GitHub only where the user can turn it off. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class)
public class HostUpdateNoticeTest {
    private static final String PC = "pc-uuid";

    private static HostCapabilities host(String version) {
        return ReflectionHelpers.callStaticMethod(HostCapabilities.class, "fromServerInfo",
                ReflectionHelpers.ClassParameter.from(String.class,
                        "<root><RustHostVersion>" + version + "</RustHostVersion></root>"));
    }

    private static void hostAnswers(String offered) throws Exception {
        JSONObject json = new JSONObject().put("current_version", "3.10.0").put("phase", "idle");
        if (offered != null) json.put("offered", offered);
        KnownUpdates.record(PC, NativePcPower.parseUpdate(json));
    }

    private Context context;

    @Before
    public void cachedGitHubRelease() {
        KnownUpdates.clear();
        context = RuntimeEnvironment.getApplication();
        context.getSharedPreferences("host_update_state", Context.MODE_PRIVATE).edit()
                .putString("latest_version", "3.11.8").apply();
    }

    @After
    public void forget() {
        KnownUpdates.clear();
    }

    @Test
    public void withoutTheHostsAnswerASelfUpdatingBuildUsesTheGitHubRelease() {
        assertEquals("3.11.8", HostUpdateChecker.availableVersion(context, PC, host("3.10.0")));
    }

    @Test
    public void theHostsOwnAnswerWinsOverGitHub() throws Exception {
        hostAnswers("3.11.7");
        assertEquals("3.11.7", HostUpdateChecker.availableVersion(context, PC, host("3.10.0")));
        hostAnswers(null);
        assertNull(HostUpdateChecker.availableVersion(context, PC, host("3.10.0")));
    }

    @Test
    public void aStoreInstallNeverShowsTheGitHubReleaseButStillTheHostsAnswer() throws Exception {
        shadowOf(context.getPackageManager()).setInstallSourceInfo(context.getPackageName(), "com.android.vending",
                "com.android.vending");
        assertNull(HostUpdateChecker.availableVersion(context, PC, host("3.10.0")));
        hostAnswers("3.11.8");
        assertEquals("3.11.8", HostUpdateChecker.availableVersion(context, PC, host("3.10.0")));
    }
}
