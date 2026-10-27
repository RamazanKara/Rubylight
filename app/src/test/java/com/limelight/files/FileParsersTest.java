package com.limelight.files;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import android.app.Application;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.HashSet;
import java.util.List;
import java.util.Set;

/** The rows, the PC's folder list and the problems, as the Rust client (protocol/android files.rs) writes them. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class FileParsersTest {
    @Test
    public void aSnapshotReadsEveryStateWithItsFields() {
        List<FileRow> rows = FileRow.parse("["
                + "{\"id\":1,\"direction\":\"upload\",\"name\":\"a.txt\",\"local_name\":\"a.txt\",\"size\":10,"
                + "\"state\":\"queued\",\"cancelling\":false,\"can_retry\":false},"
                + "{\"id\":2,\"direction\":\"download\",\"name\":\"b.bin\",\"local_name\":\"b (1).bin\",\"size\":200,"
                + "\"state\":\"running\",\"done\":50,\"total\":200,\"cancelling\":true,\"can_retry\":false},"
                + "{\"id\":3,\"direction\":\"upload\",\"name\":\"c_.jpg\",\"local_name\":\"c?.jpg\",\"size\":5,"
                + "\"state\":\"done\",\"saved_name\":\"c_ (1).jpg\",\"done\":5,\"total\":5,\"can_retry\":false},"
                + "{\"id\":4,\"direction\":\"upload\",\"name\":\"d\",\"local_name\":\"d\",\"size\":5,\"state\":\"failed\","
                + "\"problem\":\"no_space\",\"console_fix\":false,\"message\":\"507\",\"can_retry\":true},"
                + "{\"id\":5,\"direction\":\"download\",\"name\":\"e\",\"local_name\":\"e\",\"size\":0,"
                + "\"state\":\"cancelled\",\"can_retry\":true}]");
        assertEquals(5, rows.size());
        FileRow queued = rows.get(0);
        assertTrue(queued.upload);
        assertEquals(FileRow.State.QUEUED, queued.state);
        assertEquals(-1, queued.percent());
        assertFalse(queued.isFinished());
        assertNull(queued.savedName);
        assertNull(queued.problem);

        FileRow running = rows.get(1);
        assertFalse(running.upload);
        assertEquals("b.bin", running.name);
        assertEquals("b (1).bin", running.localName);
        assertEquals(25, running.percent());
        assertTrue(running.cancelling);

        FileRow done = rows.get(2);
        assertEquals("c_ (1).jpg", done.savedName);
        assertEquals(100, done.percent());
        assertTrue(done.isFinished());

        FileRow failed = rows.get(3);
        assertEquals(FileProblems.NO_SPACE, failed.problem);
        assertEquals("507", failed.message);
        assertTrue(failed.canRetry);
        assertTrue(failed.isFinished());

        assertEquals(FileRow.State.CANCELLED, rows.get(4).state);
        assertTrue(rows.get(4).isFinished());
    }

    @Test
    public void aRunningTransferOfUnknownLengthHasNoPercentAndNeverPassesAHundred() {
        FileRow unknown = FileRow.parse("[{\"id\":1,\"direction\":\"download\",\"name\":\"a\",\"local_name\":\"a\","
                + "\"size\":0,\"state\":\"running\",\"done\":4096,\"total\":0}]").get(0);
        assertEquals(-1, unknown.percent());
        FileRow over = FileRow.parse("[{\"id\":1,\"direction\":\"download\",\"name\":\"a\",\"local_name\":\"a\","
                + "\"size\":10,\"state\":\"running\",\"done\":30,\"total\":10}]").get(0);
        assertEquals(100, over.percent());
    }

    @Test
    public void anUnreadableSnapshotIsABugAndThrows() {
        for (String broken : new String[] {"{}", "not json",
                "[{\"id\":1,\"direction\":\"upload\",\"name\":\"a\",\"local_name\":\"a\",\"size\":1,\"state\":\"paused\"}]",
                "[{\"id\":1,\"direction\":\"upload\",\"state\":\"queued\"}]"}) {
            try {
                FileRow.parse(broken);
                fail(broken);
            } catch (IllegalStateException expected) {
                // Loud: the bridge and this screen disagree.
            }
        }
    }

    @Test
    public void aRunAnswerIsItsRowOrNullWhenTheListWasGone() {
        FileRow row = FileRow.ofRun("{\"ok\":{\"id\":7,\"direction\":\"upload\",\"name\":\"a\",\"local_name\":\"a\","
                + "\"size\":1,\"state\":\"done\",\"saved_name\":\"a\"}}");
        assertEquals(7, row.id);
        assertEquals(FileRow.State.DONE, row.state);
        assertNull(FileRow.ofRun("{\"error\":{\"problem\":\"local\",\"message\":\"no such transfer\"}}"));
        try {
            FileRow.ofRun("[]");
            fail();
        } catch (IllegalStateException expected) {
            // Not an answer the bridge writes.
        }
    }

    @Test
    public void theFolderListReadsItsFilesInOrder() {
        RemoteFiles list = RemoteFiles.parse("{\"ok\":{\"folder\":\"C:\\\\Users\\\\Me\\\\Downloads\\\\Rubylight\","
                + "\"free_bytes\":1000,\"max_file_bytes\":500,\"files\":["
                + "{\"name\":\"b.txt\",\"size\":3,\"modified_unix_ms\":2000},"
                + "{\"name\":\"a.txt\",\"size\":4,\"modified_unix_ms\":1000}]}}");
        assertNull(list.problem);
        assertEquals("C:\\Users\\Me\\Downloads\\Rubylight", list.folder);
        assertEquals(1000, list.freeBytes);
        assertEquals(500, list.maxFileBytes);
        assertEquals(2, list.files.size());
        assertEquals("b.txt", list.files.get(0).name);
        assertEquals(3, list.files.get(0).size);
        assertEquals(2000, list.files.get(0).modifiedUnixMs);
        assertEquals("a.txt", list.files.get(1).name);
    }

    @Test
    public void aRefusedListKeepsTheProblemAndAnUnreadableOneIsTheHosts() {
        RemoteFiles refused = RemoteFiles.parse("{\"error\":{\"problem\":\"no_permission\",\"message\":\"403\","
                + "\"http_status\":403,\"code\":\"file_download_permission\"}}");
        assertEquals(FileProblems.NO_PERMISSION, refused.problem);
        assertEquals("403", refused.message);
        assertTrue(refused.files.isEmpty());
        for (String broken : new String[] {"garbage", "{}", "{\"ok\":{\"folder\":\"x\"}}",
                "{\"error\":{\"message\":\"no problem id\"}}"}) {
            RemoteFiles list = RemoteFiles.parse(broken);
            assertEquals(broken, FileProblems.HOST, list.problem);
            assertTrue(broken, list.files.isEmpty());
        }
    }

    @Test
    public void everyProblemOfTheRustListHasItsOwnTextAndAnUnknownOneReadsAsTheHosts() {
        // nightjar-core's Problem::id, in its order, then the cancel that run() reports.
        String[] ids = {"no_permission", "transfer_off", "invalid_name", "interrupted", "too_large", "no_space",
                "no_console_user", "busy", "not_found", "network", "local", "host", "cancelled"};
        Set<Integer> texts = new HashSet<>();
        for (String id : ids) texts.add(FileProblems.text(id));
        assertEquals("one text each", ids.length, texts.size());
        assertEquals(R.string.files_problem_host, FileProblems.text("a_future_problem"));
        assertEquals(R.string.files_problem_host, FileProblems.text(null));
        assertEquals(R.string.files_cancelled, FileProblems.text(FileProblems.CANCELLED));
    }

    private static HostCapabilities withFiles(String features, String permissions) throws Exception {
        return NativeApiFixtures.NATIVE_CAPS.withNativeCapabilities("{\"api_version\":1,"
                + "\"host\":{\"name\":\"PC\",\"version\":\"3.0.0\",\"unique_id\":\"pc\"},\"features\":[" + features + "],"
                + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true,\"host_control\":false" + permissions + "}}");
    }

    @Test
    public void accessFollowsTheHostsFeaturesThenThisDevicesPermission() throws Exception {
        HostCapabilities both = withFiles("\"files.upload\",\"files.download\"",
                ",\"file_upload\":true,\"file_download\":false");
        assertEquals(FileProblems.Access.READY, FileProblems.access(both, true));
        assertEquals(FileProblems.Access.NO_PERMISSION, FileProblems.access(both, false));
        assertEquals(R.string.files_no_download_permission,
                FileProblems.accessText(FileProblems.access(both, false), false));
        assertEquals(0, FileProblems.accessText(FileProblems.Access.READY, true));

        HostCapabilities off = withFiles("", ",\"file_upload\":true,\"file_download\":true");
        assertEquals(FileProblems.Access.OFF, FileProblems.access(off, true));
        assertEquals(FileProblems.Access.OFF, FileProblems.access(off, false));
    }

    @Test
    public void accessIsUnknownUntilTheNativeApiAnsweredAndUnavailableWithoutIt() {
        assertEquals(FileProblems.Access.CHECKING, FileProblems.access(NativeApiFixtures.NATIVE_CAPS, true));
        assertEquals(FileProblems.Access.UNAVAILABLE,
                FileProblems.access(TestHosts.fixture("sunshine-serverinfo.xml"), true));
        assertEquals(FileProblems.Access.UNAVAILABLE, FileProblems.access(HostCapabilities.UNKNOWN, false));
    }
}
