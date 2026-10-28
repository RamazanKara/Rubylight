package com.limelight.files;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;

import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

/** What the file screen takes from a share and how it words a transfer's row. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class FileTransferScreenTest {
    private final Context context = RuntimeEnvironment.getApplication();

    private static FileRow row(String json) {
        return FileRow.parse("[" + json + "]").get(0);
    }

    @Test
    public void aShareOfOneOrSeveralFilesKeepsTheirOrderAndAnythingElseBringsNone() {
        Uri a = Uri.parse("content://a/1");
        Uri b = Uri.parse("content://b/2");
        Intent one = new Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, a);
        assertEquals(Collections.singletonList(a), FileTransferActivity.sharedUris(one));
        Intent several = new Intent(Intent.ACTION_SEND_MULTIPLE)
                .putParcelableArrayListExtra(Intent.EXTRA_STREAM, new ArrayList<>(Arrays.asList(b, null, a)));
        assertEquals(Arrays.asList(b, a), FileTransferActivity.sharedUris(several));
        // A shared text has no file; opening from a menu has no share.
        assertTrue(FileTransferActivity.sharedUris(new Intent(Intent.ACTION_SEND)
                .putExtra(Intent.EXTRA_TEXT, "hello")).isEmpty());
        assertTrue(FileTransferActivity.sharedUris(FileTransferActivity.forPc(context, "pc")).isEmpty());
    }

    @Test
    public void theMenusOpenTheScreenForTheirPc() {
        Intent intent = FileTransferActivity.forPc(context, "tower");
        assertEquals(FileTransferActivity.class.getName(), intent.getComponent().getClassName());
        assertEquals("tower", intent.getStringExtra(FileTransferActivity.EXTRA_PC_UUID));
        assertEquals("tower", FileTransferActivity.chosenPc(intent));
    }

    @Test
    public void aShareAlwaysAsksWhichPcAndSendsOnlyAnotherAppsFiles() {
        // Another app naming the PC would send without the user choosing it.
        for (String action : new String[] {Intent.ACTION_SEND, Intent.ACTION_SEND_MULTIPLE}) {
            Intent share = FileTransferActivity.forPc(context, "tower").setAction(action)
                    .putExtra(Intent.EXTRA_STREAM, Uri.parse("content://media/1"));
            assertNull(action, FileTransferActivity.chosenPc(share));
        }
        String own = "com.butterpollo.client";
        assertTrue(FileTransferActivity.sendable(Uri.parse("content://media/external/1"), own, "com.android.providers.media"));
        // Served by nobody: it is let through and fails to open as a local problem.
        assertTrue(FileTransferActivity.sendable(Uri.parse("content://gone/1"), own, null));
        assertFalse(FileTransferActivity.sendable(Uri.parse("content://com.butterpollo.client.reports/r/log.txt"), own, own));
        assertFalse(FileTransferActivity.sendable(
                Uri.parse("file:///data/data/com.butterpollo.client/shared_prefs/settings.xml"), own, null));
        assertFalse(FileTransferActivity.sendable(Uri.parse("android.resource://com.butterpollo.client/1"), own, null));
        assertFalse(FileTransferActivity.sendable(Uri.parse("content:///no-authority"), own, null));
    }

    @Test
    public void eachRowSaysWhereItIs() {
        String head = "\"direction\":\"upload\",\"name\":\"a_.txt\",\"local_name\":\"a?.txt\",\"size\":2048,";
        assertEquals(context.getString(R.string.files_queued),
                FileTransferActivity.describe(context, row("{\"id\":1," + head + "\"state\":\"queued\"}")));
        String sending = FileTransferActivity.describe(context,
                row("{\"id\":1," + head + "\"state\":\"running\",\"done\":1024,\"total\":2048}"));
        assertTrue(sending, sending.contains("50%"));
        assertEquals(context.getString(R.string.files_cancelling), FileTransferActivity.describe(context,
                row("{\"id\":1," + head + "\"state\":\"running\",\"done\":1,\"total\":2048,\"cancelling\":true}")));
        assertEquals(context.getString(R.string.files_sent), FileTransferActivity.describe(context,
                row("{\"id\":1," + head + "\"state\":\"done\",\"saved_name\":\"a_.txt\"}")));
        assertEquals(context.getString(R.string.files_sent_as, "a_ (1).txt"), FileTransferActivity.describe(context,
                row("{\"id\":1," + head + "\"state\":\"done\",\"saved_name\":\"a_ (1).txt\"}")));
        assertEquals(context.getString(R.string.files_problem_no_space), FileTransferActivity.describe(context,
                row("{\"id\":1," + head + "\"state\":\"failed\",\"problem\":\"no_space\",\"message\":\"507\"}")));
        assertEquals(context.getString(R.string.files_cancelled), FileTransferActivity.describe(context,
                row("{\"id\":1," + head + "\"state\":\"cancelled\"}")));
    }

    @Test
    public void aDownloadWithoutALengthCountsBytesAndALandedOneIsSaved() {
        String head = "\"direction\":\"download\",\"name\":\"b\",\"local_name\":\"b\",\"size\":0,";
        String unknown = FileTransferActivity.describe(context,
                row("{\"id\":2," + head + "\"state\":\"running\",\"done\":4096,\"total\":0}"));
        assertTrue(unknown, unknown.startsWith(context.getString(R.string.files_receiving_unknown, "").trim()));
        assertEquals(context.getString(R.string.files_saved), FileTransferActivity.describe(context,
                row("{\"id\":2," + head + "\"state\":\"done\",\"saved_name\":\"b\"}")));
    }

    @Test
    public void everyStringHasItsGermanText() {
        List<String> missing = new ArrayList<>();
        Context german = context.createConfigurationContext(configuration("de"));
        for (int id : new int[] {R.string.files_menu, R.string.files_title, R.string.files_share_label,
                R.string.files_open, R.string.files_stream_leaves, R.string.files_no_upload_permission}) {
            if (german.getString(id).equals(context.getString(id))) missing.add(context.getResources().getResourceEntryName(id));
        }
        assertTrue(missing.toString(), missing.isEmpty());
    }

    private android.content.res.Configuration configuration(String language) {
        android.content.res.Configuration configuration =
                new android.content.res.Configuration(context.getResources().getConfiguration());
        configuration.setLocale(new java.util.Locale(language));
        return configuration;
    }
}
