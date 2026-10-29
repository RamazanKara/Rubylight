package com.limelight.utils;

import android.app.Application;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ProviderInfo;
import android.net.Uri;
import android.provider.DocumentsContract;

import com.limelight.AppView;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowContentResolver;

import java.io.File;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 25, application = Application.class)
public class FrontendSyncTest {
    private static final String AUTHORITY = "com.limelight.test.documents";
    private static final int ACCESS = Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION;
    @Rule public TemporaryFolder temporary = new TemporaryFolder();
    private Context context;
    private ComputerDetails computer;
    private Uri romsTree;
    private Uri esdeTree;
    private FrontendExportStore store;

    @Before
    public void setup() throws Exception {
        context = RuntimeEnvironment.getApplication();
        FrontendExporterTest.TestDocuments provider = new FrontendExporterTest.TestDocuments();
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
        context.getContentResolver().takePersistableUriPermission(romsTree, ACCESS);
        context.getContentResolver().takePersistableUriPermission(esdeTree, ACCESS);
        computer = new ComputerDetails();
        computer.uuid = "host";
        computer.name = "PC";
        store = new FrontendExportStore(context, computer.uuid, FrontendLauncher.ES_DE);
    }

    private File file(String relative) {
        return new File(temporary.getRoot(), relative);
    }

    private void hostGames(String... games) throws Exception {
        StringBuilder xml = new StringBuilder("<root status_code=\"200\">");
        for (int i = 0; i < games.length; i++) {
            xml.append("<App><AppTitle>").append(games[i]).append("</AppTitle><ID>").append(i + 1).append("</ID></App>");
        }
        xml.append("</root>");
        try (OutputStream out = CacheHelper.openCacheFileForOutput(context.getCacheDir(), "applist", computer.uuid)) {
            out.write(xml.toString().getBytes(StandardCharsets.UTF_8));
        }
    }

    private void cover(int appId, byte value) throws Exception {
        File cover = CacheHelper.openPath(true, context.getCacheDir(), "boxart", computer.uuid, appId + ".png");
        Files.write(cover.toPath(), new byte[] {value});
    }

    private java.util.List<FrontendLauncher> sync() {
        return FrontendSync.sync(context, computer, new FrontendExporter(context, computer));
    }

    /** The export by hand that turns syncing on. */
    private void exported() throws Exception {
        FrontendExporter exporter = new FrontendExporter(context, computer);
        java.util.List<NvApp> apps = exporter.loadApps();
        java.util.Map<String, String> covers = store.covers(esdeTree);
        exporter.export(apps, romsTree, esdeTree, covers);
        store.save(romsTree, esdeTree, FrontendSync.fingerprint(computer.name, apps), covers);
    }

    private String[] entries() {
        String[] names = file("roms/butterpollo").list();
        Arrays.sort(names);
        return names;
    }

    @Test
    public void anUnchangedGameListExportsNothing() throws Exception {
        hostGames("Game", "Racing");
        exported();
        File game = file("roms/butterpollo/Game.art");
        Files.writeString(game.toPath(), "edited");

        assertTrue(sync().isEmpty());

        assertEquals("edited", Files.readString(game.toPath()));
    }

    @Test
    public void anAddedGameIsExportedWithItsCover() throws Exception {
        hostGames("Game");
        exported();
        hostGames("Game", "Racing");
        cover(2, (byte) 7);

        assertTrue(sync().isEmpty());

        assertArrayEquals(new String[] {"Game.art", "Racing.art"}, entries());
        assertArrayEquals(new byte[] {7}, Files.readAllBytes(file("esde/downloaded_media/butterpollo/covers/Racing.png").toPath()));
        // Synced once, the same list is not written again.
        Files.writeString(file("roms/butterpollo/Racing.art").toPath(), "edited");
        sync();
        assertEquals("edited", Files.readString(file("roms/butterpollo/Racing.art").toPath()));
    }

    @Test
    public void aRemovedGameLeavesWithItsCover() throws Exception {
        hostGames("Game", "Racing");
        cover(2, (byte) 7);
        exported();
        assertTrue(file("esde/downloaded_media/butterpollo/covers/Racing.png").isFile());
        hostGames("Game");

        sync();

        assertArrayEquals(new String[] {"Game.art"}, entries());
        assertFalse(file("esde/downloaded_media/butterpollo/covers/Racing.png").exists());
    }

    @Test
    public void aHiddenGameLeavesTheLauncher() throws Exception {
        hostGames("Game", "Racing");
        exported();
        context.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, Context.MODE_PRIVATE).edit()
                .putStringSet(computer.uuid, new HashSet<>(Collections.singleton("2"))).apply();

        sync();

        assertArrayEquals(new String[] {"Game.art"}, entries());
    }

    @Test
    public void aRenamedGameKeepsItsFile() throws Exception {
        hostGames("Game");
        exported();
        hostGames("Game Deluxe");

        sync();

        assertArrayEquals(new String[] {"Game.art"}, entries());
        assertTrue(Files.readString(file("roms/butterpollo/Game.art").toPath()).contains("[app_name] Game Deluxe"));
    }

    @Test
    public void aLostFolderStopsSyncingLoudlyUntilAnExportByHand() throws Exception {
        hostGames("Game");
        exported();
        context.getContentResolver().releasePersistableUriPermission(romsTree, ACCESS);
        hostGames("Game", "Racing");

        assertEquals(Collections.singletonList(FrontendLauncher.ES_DE), sync());
        assertTrue(store.failed());
        assertArrayEquals(new String[] {"Game.art"}, entries());
        // Stopped: later changes are not tried again behind the user's back.
        assertTrue(sync().isEmpty());

        context.getContentResolver().takePersistableUriPermission(romsTree, ACCESS);
        exported();
        assertFalse(store.failed());
    }

    @Test
    public void stoppingForgetsTheFolderAndLeavesItsFiles() throws Exception {
        hostGames("Game");
        exported();
        store.forget();
        hostGames("Racing");

        assertTrue(sync().isEmpty());

        assertNull(store.romsTree());
        assertArrayEquals(new String[] {"Game.art"}, entries());
        assertTrue(FrontendExportStore.exported(context, computer.uuid).isEmpty());
    }

    @Test
    public void noCachedGameListNeverEmptiesTheLauncher() throws Exception {
        hostGames("Game");
        exported();
        CacheHelper.deleteCacheFile(context.getCacheDir(), "applist", computer.uuid);

        sync();

        assertArrayEquals(new String[] {"Game.art"}, entries());
    }

    @Test
    public void fingerprintFollowsTheGamesNotTheirOrder() {
        NvApp game = new NvApp("Game", 1, false);
        NvApp racing = new NvApp("Racing", 2, false);
        assertEquals(FrontendSync.fingerprint("PC", Arrays.asList(game, racing)),
                FrontendSync.fingerprint("PC", Arrays.asList(racing, game)));
        assertNotEquals(FrontendSync.fingerprint("PC", Arrays.asList(game, racing)),
                FrontendSync.fingerprint("PC", Collections.singletonList(game)));
        assertNotEquals(FrontendSync.fingerprint("PC", Collections.singletonList(game)),
                FrontendSync.fingerprint("PC", Collections.singletonList(new NvApp("Game 2", 1, false))));
        assertNotEquals(FrontendSync.fingerprint("PC", Collections.singletonList(game)),
                FrontendSync.fingerprint("Other PC", Collections.singletonList(game)));
    }
}
