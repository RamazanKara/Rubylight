package com.limelight.update;

import android.app.Activity;
import android.app.Application;
import android.content.Context;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;

import java.util.Collections;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class UpdateNoticesTest {
    private static ReleaseInfo release(String version, boolean prerelease) {
        return new ReleaseInfo(version, "", false, prerelease, Collections.emptyList());
    }

    private static HostCapabilities host(String version) {
        return ReflectionHelpers.callStaticMethod(HostCapabilities.class, "fromServerInfo",
                ReflectionHelpers.ClassParameter.from(String.class,
                        "<root><RustHostVersion>" + version + "</RustHostVersion></root>"));
    }

    @Test public void cachedAppUpdatesRespectInstalledAndSkippedVersions() {
        UpdatePrefs prefs = new UpdatePrefs(RuntimeEnvironment.getApplication());
        prefs.recordRelease(release("v0.8.0", false), 42);
        assertEquals(42, prefs.lastCheck());
        assertEquals("0.8.0", new UpdatePrefs(RuntimeEnvironment.getApplication()).availableVersion("0.7.17"));
        assertNull(prefs.availableVersion("0.8.0"));
        prefs.skip("v0.8.0");
        assertNull(prefs.availableVersion("0.7.17"));
        prefs.recordRelease(release("v0.9.0", false), 43);
        assertEquals("0.9.0", prefs.availableVersion("0.7.17"));
        prefs.recordRelease(release("v1.0.0-rc.1", true), 44);
        assertNull(prefs.availableVersion("0.7.17"));
    }

    @Test public void hostNoticeUsesEachPcsInstalledVersionAndOnlyIdentifiedRubylightHosts() {
        Context context = RuntimeEnvironment.getApplication();
        context.getSharedPreferences("host_update_state", Context.MODE_PRIVATE).edit()
                .putString("latest_version", "2.5.11").apply();
        assertEquals("2.5.11", HostUpdateChecker.availableVersion(context, host("2.5.10")));
        assertNull(HostUpdateChecker.availableVersion(context, host("2.5.11")));
        assertNull(HostUpdateChecker.availableVersion(context, host("2.6.0-rc.1")));
        assertNull(HostUpdateChecker.availableVersion(context, host("unknown")));
        assertNull(HostUpdateChecker.availableVersion(context, HostCapabilities.UNKNOWN));
    }

    @Test public void directLaunchShowsBothCachedUpdatesWithoutAnotherNetworkCheck() throws Exception {
        try (ActivityController<Activity> activity = Robolectric.buildActivity(Activity.class).create()) {
            long now = System.currentTimeMillis();
            new UpdatePrefs(activity.get()).recordRelease(release("99.0.0", false), now);
            activity.get().getSharedPreferences("host_update_state", Context.MODE_PRIVATE).edit()
                    .putString("latest_version", "2.5.11").putLong("last_check", now).apply();
            ComputerDetails details = new ComputerDetails();
            details.caps = host("2.5.10");
            CompletableFuture<String> notice = new CompletableFuture<>();
            UpdateFlow.notifyForStream(activity.get(), details, () -> true, notice::complete);
            long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(3);
            while (!notice.isDone() && System.nanoTime() < deadline) {
                ShadowLooper.idleMainLooper();
                Thread.sleep(10);
            }
            String text = notice.get(1, TimeUnit.SECONDS);
            assertTrue(text.contains("Nightjar 99.0.0"));
            assertTrue(text.contains("Rubylight 2.5.11"));
        }
    }

    @Test public void noNoticeIsBuiltWhenBothVersionsAreCurrent() {
        assertEquals("", UpdateFlow.streamUpdateMessage(RuntimeEnvironment.getApplication(), null, null));
    }
}
