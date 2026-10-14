package com.limelight.utils;

import android.app.Application;
import android.content.Context;
import android.content.pm.ProviderInfo;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.CancellationSignal;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import android.provider.DocumentsContract.Document;
import android.provider.DocumentsProvider;

import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.rules.TemporaryFolder;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowContentResolver;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.nio.file.Files;
import java.util.Collections;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 25, application = Application.class)
public class FrontendExporterTest {
    private static final String AUTHORITY = "com.limelight.test.documents";
    @Rule public TemporaryFolder temporary = new TemporaryFolder();
    private Context context;
    private TestDocuments provider;
    private ComputerDetails computer;
    private FrontendExporter exporter;
    private Uri romsTree;
    private Uri esdeTree;
    private NvApp app;

    @Before
    public void setup() throws Exception {
        context = RuntimeEnvironment.getApplication();
        provider = new TestDocuments();
        provider.root = temporary.getRoot();
        ProviderInfo info = new ProviderInfo();
        info.authority = AUTHORITY;
        info.exported = true;
        info.grantUriPermissions = true;
        info.readPermission = "android.permission.MANAGE_DOCUMENTS";
        info.writePermission = "android.permission.MANAGE_DOCUMENTS";
        provider.attachInfo(context, info);
        ShadowContentResolver.registerProviderInternal(AUTHORITY, provider);
        temporary.newFolder("roms", "butterpollo");
        temporary.newFolder("esde");
        romsTree = DocumentsContract.buildTreeDocumentUri(AUTHORITY, "roms");
        esdeTree = DocumentsContract.buildTreeDocumentUri(AUTHORITY, "esde");
        computer = new ComputerDetails();
        computer.uuid = "host";
        computer.name = "PC";
        app = new NvApp("Game", 42, false);
        exporter = new FrontendExporter(context, computer);
    }

    private File file(String relative) {
        return new File(temporary.getRoot(), relative);
    }

    private String entry(String host, String name, int appId) {
        return FrontendEntry.serialize(host, "PC", null, name, appId);
    }

    private void cacheCover() throws Exception {
        File cover = CacheHelper.openPath(true, context.getCacheDir(), "boxart", computer.uuid,
                app.getAssetCacheKey() + ".png");
        Files.write(cover.toPath(), new byte[] {1, 2, 3});
    }

    @Test
    public void renamedAppUpdatesItsExistingEntryWithoutCreatingADuplicate() throws Exception {
        File existing = file("roms/butterpollo/Old name.ART");
        Files.writeString(existing.toPath(), entry("host", "Old name", 42));
        cacheCover();

        assertEquals(1, exporter.export(Collections.singletonList(app), romsTree, esdeTree));

        assertArrayEquals(new String[] {"Old name.ART"}, existing.getParentFile().list());
        assertEquals(entry("host", "Game", 42), Files.readString(existing.toPath()));
        assertTrue(file("esde/downloaded_media/butterpollo/covers/Old name.png").isFile());
        String systems = Files.readString(file("esde/custom_systems/es_systems.xml").toPath());
        assertTrue(systems.contains("<name>butterpollo</name>"));
        assertTrue(systems.contains("<fullname>Nightjar</fullname>"));
    }

    @Test
    public void stableUuidMatchesEvenWhenNameAndNumericIdChange() throws Exception {
        File existing = file("roms/butterpollo/Original.art");
        Files.writeString(existing.toPath(), FrontendEntry.serialize("HOST", "PC", "APP-UUID", "Original", 7));
        app.setAppUuid("app-uuid");
        exporter.export(Collections.singletonList(app), romsTree, null);
        assertArrayEquals(new String[] {"Original.art"}, existing.getParentFile().list());
        assertTrue(Files.readString(existing.toPath()).contains("[app_id] 42"));
    }

    @Test
    public void differentUuidDoesNotOverwriteAnAppThatReusedItsNumericId() throws Exception {
        File existing = file("roms/butterpollo/Game.art");
        String original = FrontendEntry.serialize("host", "PC", "old-uuid", "Game", 42);
        Files.writeString(existing.toPath(), original);
        app.setAppUuid("new-uuid");
        exporter.export(Collections.singletonList(app), romsTree, null);
        assertEquals(original, Files.readString(existing.toPath()));
        assertTrue(file("roms/butterpollo/Game (PC).art").isFile());
    }

    @Test
    public void failedNewEsDeConfigurationIsRemoved() {
        provider.failWriteSuffix = ".xml";
        assertThrows(IOException.class,
                () -> exporter.export(Collections.singletonList(app), romsTree, esdeTree));
        assertEquals(0, file("esde/custom_systems").list().length);
    }

    @Test
    public void failedNewEntryIsRemovedSoRetryUsesTheOriginalName() throws Exception {
        provider.failWriteSuffix = ".art";
        assertThrows(IOException.class,
                () -> exporter.export(Collections.singletonList(app), romsTree, null));
        assertEquals(0, file("roms/butterpollo").list().length);

        provider.failWriteSuffix = null;
        exporter.export(Collections.singletonList(app), romsTree, null);
        assertArrayEquals(new String[] {"Game.art"}, file("roms/butterpollo").list());
    }

    @Test
    public void failedExistingEntryIsNotDeleted() throws Exception {
        File existing = file("roms/butterpollo/Game.art");
        String original = entry("host", "Game", 42);
        Files.writeString(existing.toPath(), original);
        provider.failWriteSuffix = ".art";

        assertThrows(IOException.class,
                () -> exporter.export(Collections.singletonList(app), romsTree, null));

        assertEquals(original, Files.readString(existing.toPath()));
    }

    @Test
    public void failedCoverCopyCanBeRetriedWithoutLeavingAnEmptyCover() throws Exception {
        cacheCover();
        provider.failWriteSuffix = ".png";
        assertEquals(1, exporter.export(Collections.singletonList(app), romsTree, esdeTree));
        File cover = file("esde/downloaded_media/butterpollo/covers/Game.png");
        assertFalse(cover.exists());

        provider.failWriteSuffix = null;
        exporter.export(Collections.singletonList(app), romsTree, esdeTree);
        assertArrayEquals(new byte[] {1, 2, 3}, Files.readAllBytes(cover.toPath()));
    }

    @Test
    public void existingEntriesAndCustomCoversFromOtherHostsArePreserved() throws Exception {
        File existing = file("roms/butterpollo/Game.art");
        String original = entry("another-host", "Game", 42);
        Files.writeString(existing.toPath(), original);
        cacheCover();

        exporter.export(Collections.singletonList(app), romsTree, esdeTree);
        assertEquals(original, Files.readString(existing.toPath()));
        assertTrue(file("roms/butterpollo/Game (PC).art").isFile());
        File cover = file("esde/downloaded_media/butterpollo/covers/Game (PC).png");
        Files.writeString(cover.toPath(), "Custom cover");
        exporter.export(Collections.singletonList(app), romsTree, esdeTree);
        assertEquals("Custom cover", Files.readString(cover.toPath()));
    }

    @Test
    public void unreadableOrMalformedEntriesAreNeverOverwritten() throws Exception {
        File existing = file("roms/butterpollo/Game.art");
        Files.writeString(existing.toPath(), "Not a game entry");
        exporter.export(Collections.singletonList(app), romsTree, null);
        assertEquals("Not a game entry", Files.readString(existing.toPath()));
        assertTrue(file("roms/butterpollo/Game (PC).art").isFile());
    }

    @Test
    public void malformedEsDeConfigurationIsPreserved() throws Exception {
        temporary.newFolder("esde", "custom_systems");
        File config = file("esde/custom_systems/es_systems.xml");
        Files.writeString(config.toPath(), "<systemList><broken>");
        assertThrows(IllegalArgumentException.class,
                () -> exporter.export(Collections.singletonList(app), romsTree, esdeTree));
        assertEquals("<systemList><broken>", Files.readString(config.toPath()));
    }

    public static class TestDocuments extends DocumentsProvider {
        File root;
        String failWriteSuffix;

        @Override public boolean onCreate() { return true; }

        @Override
        public boolean isChildDocument(String parentDocumentId, String documentId) {
            return documentId.startsWith(parentDocumentId + "/");
        }

        @Override
        public Cursor queryRoots(String[] projection) {
            return new MatrixCursor(projection == null ? new String[] {"root_id"} : projection);
        }

        private MatrixCursor documents(String[] projection) {
            return new MatrixCursor(projection == null ? new String[] {
                    Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE
            } : projection);
        }

        private void addDocument(MatrixCursor cursor, String documentId) {
            File document = new File(root, documentId);
            MatrixCursor.RowBuilder row = cursor.newRow();
            for (String column : cursor.getColumnNames()) {
                if (Document.COLUMN_DOCUMENT_ID.equals(column)) row.add(documentId);
                else if (Document.COLUMN_DISPLAY_NAME.equals(column)) row.add(document.getName());
                else if (Document.COLUMN_MIME_TYPE.equals(column)) row.add(document.isDirectory()
                        ? Document.MIME_TYPE_DIR : "application/octet-stream");
                else row.add(null);
            }
        }

        @Override
        public Cursor queryDocument(String documentId, String[] projection) {
            MatrixCursor cursor = documents(projection);
            addDocument(cursor, documentId);
            return cursor;
        }

        @Override
        public Cursor queryChildDocuments(String parentDocumentId, String[] projection, String sortOrder) {
            MatrixCursor cursor = documents(projection);
            File[] children = new File(root, parentDocumentId).listFiles();
            if (children != null) {
                for (File child : children) addDocument(cursor, parentDocumentId + "/" + child.getName());
            }
            return cursor;
        }

        @Override
        public String createDocument(String parentDocumentId, String mimeType, String displayName)
                throws FileNotFoundException {
            String documentId = parentDocumentId + "/" + displayName;
            File document = new File(root, documentId);
            try {
                boolean created = Document.MIME_TYPE_DIR.equals(mimeType) ? document.mkdir() : document.createNewFile();
                if (!created) throw new FileNotFoundException("Document already exists");
                return documentId;
            } catch (IOException failure) {
                throw new FileNotFoundException(failure.getMessage());
            }
        }

        @Override
        public void deleteDocument(String documentId) throws FileNotFoundException {
            if (!new File(root, documentId).delete()) throw new FileNotFoundException(documentId);
        }

        @Override
        public ParcelFileDescriptor openDocument(String documentId, String mode, CancellationSignal signal)
                throws FileNotFoundException {
            if (mode.contains("w") && failWriteSuffix != null && documentId.endsWith(failWriteSuffix)) {
                throw new FileNotFoundException("Simulated provider write failure");
            }
            return ParcelFileDescriptor.open(new File(root, documentId), ParcelFileDescriptor.parseMode(mode));
        }
    }
}
