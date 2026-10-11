package com.limelight.binding.video;

import android.app.Application;
import android.content.Context;
import android.content.ContextWrapper;

import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.junit.rules.TemporaryFolder;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GpuDriverStoreTest {
    @Rule public TemporaryFolder storage = new TemporaryFolder();
    private Context context;
    private static final String METADATA = "{\"schemaVersion\":1,\"name\":\"Turnip test\",\"author\":\"Mesa\","
            + "\"packageVersion\":\"1\",\"vendor\":\"Mesa\",\"driverVersion\":\"25.0.0\",\"minApi\":29,"
            + "\"libraryName\":\"libvulkan_freedreno.so\"}";

    @Before
    public void setup() {
        context = new ContextWrapper(RuntimeEnvironment.getApplication()) {
            @Override public File getNoBackupFilesDir() { return storage.getRoot(); }
        };
        context.getSharedPreferences("gpu-drivers", Context.MODE_PRIVATE).edit().clear().commit();
    }

    @Test
    public void importSelectSystemAndRemoveKeepFilesAppPrivate() throws Exception {
        GpuDriverStore.Driver first = install(packageFiles());
        assertEquals("Turnip test", first.name);
        assertTrue(first.directory.getCanonicalPath().startsWith(storage.getRoot().getCanonicalPath() + File.separator));
        assertEquals(1, GpuDriverStore.list(context).size());
        assertNull(GpuDriverStore.selected(context));
        GpuDriverStore.select(context, first);
        assertEquals(first.id, GpuDriverStore.selected(context).id);
        GpuDriverStore.select(context, null);
        assertNull(GpuDriverStore.selected(context));
        assertTrue(new File(first.directory, first.libraryName).isFile());
        GpuDriverStore.select(context, first);
        GpuDriverStore.remove(context, first);
        assertNull(GpuDriverStore.selected(context));
        assertTrue(GpuDriverStore.list(context).isEmpty());
        assertFalse(first.directory.exists());
    }

    @Test
    public void rejectsTraversalAbsoluteAndNestedPathsAndLeavesExistingSelection() throws Exception {
        GpuDriverStore.Driver existing = install(packageFiles());
        GpuDriverStore.select(context, existing);
        for (String unsafe : new String[] {"../outside.so", "/outside.so", "folder/driver.so", "folder\\driver.so", "C:driver.so"}) {
            Map<String, byte[]> files = packageFiles();
            files.put(unsafe, elf());
            assertThrows(unsafe, IOException.class, () -> install(files));
        }
        assertEquals(existing.id, GpuDriverStore.selected(context).id);
        assertEquals(1, new File(storage.getRoot(), "gpu-drivers").list().length);
        assertFalse(new File(storage.getRoot(), "outside.so").exists());
    }

    @Test
    public void validatesMetadataAndEverySharedLibraryAbi() throws Exception {
        for (String metadata : new String[] {METADATA.replace("\"schemaVersion\":1", "\"schemaVersion\":2"),
                METADATA.replace("\"schemaVersion\":1", "\"schemaVersion\":1.5"),
                METADATA.replace("\"minApi\":29", "\"minApi\":35"),
                METADATA.replace("libvulkan_freedreno.so", "../outside.so"),
                METADATA.replace("\"vendor\":\"Mesa\"", "\"vendor\":\"Other\""), "{}"}) {
            Map<String, byte[]> files = packageFiles();
            files.put("meta.json", metadata.getBytes(StandardCharsets.UTF_8));
            assertThrows(IOException.class, () -> install(files));
        }
        Map<String, byte[]> files = packageFiles();
        byte[] wrongAbi = elf();
        wrongAbi[18] = 62;
        files.put("dependency.so", wrongAbi);
        assertThrows(IOException.class, () -> install(files));
        files.remove("dependency.so");
        files.put("libvulkan_freedreno.so", new byte[8]);
        assertThrows(IOException.class, () -> install(files));
        assertTrue(GpuDriverStore.list(context).isEmpty());
        assertEquals(0, new File(storage.getRoot(), "gpu-drivers").list().length);
    }

    @Test
    public void boundsMetadataAndFileCount() throws Exception {
        Map<String, byte[]> oversizedMetadata = packageFiles();
        oversizedMetadata.put("meta.json", new byte[16385]);
        assertThrows(IOException.class, () -> install(oversizedMetadata));
        Map<String, byte[]> files = packageFiles();
        for (int i = 0; i < 64; i++) files.put("notice" + i + ".txt", new byte[0]);
        Map<String, byte[]> tooMany = files;
        assertThrows(IOException.class, () -> install(tooMany));
        assertEquals(0, new File(storage.getRoot(), "gpu-drivers").list().length);
    }

    @Test
    public void rejectsCompressedPackagesOverTheExpandedSizeLimit() throws Exception {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        try (ZipOutputStream zip = new ZipOutputStream(bytes)) {
            zip.putNextEntry(new ZipEntry("oversized.so"));
            zip.write(elf());
            byte[] block = new byte[65536];
            for (long size = 0; size < GpuDriverStore.MAX_UNPACKED_BYTES; size += block.length) zip.write(block);
        }
        assertTrue(bytes.size() < 1024 * 1024);
        assertThrows(IOException.class, () -> GpuDriverStore.importDriver(context, new ByteArrayInputStream(bytes.toByteArray())));
        assertEquals(0, new File(storage.getRoot(), "gpu-drivers").list().length);
    }

    @Test
    public void loaderFailurePreservesSelectionAndInterruptedProcessResetsToSystem() throws Exception {
        GpuDriverStore.Driver driver = install(packageFiles());
        GpuDriverStore.select(context, driver);
        assertTrue(GpuDriverStore.beginUse(context, driver));
        assertEquals(driver.id, GpuDriverStore.selected(context).id);
        assertThrows(IOException.class, () -> GpuDriverStore.remove(context, driver));
        GpuDriverStore.endUse(context, driver, true);
        assertTrue(GpuDriverStore.usedFallback(context));
        assertEquals(driver.id, GpuDriverStore.selected(context).id);
        assertTrue(GpuDriverStore.beginUse(context, driver));
        GpuDriverStore.endUse(context, driver, false);
        assertFalse(GpuDriverStore.usedFallback(context));
        context.getSharedPreferences("gpu-drivers", Context.MODE_PRIVATE).edit().putString("active", driver.id).commit();
        assertNull(GpuDriverStore.selected(context));
        assertTrue(GpuDriverStore.usedFallback(context));
        assertEquals(1, GpuDriverStore.list(context).size());
        GpuDriverStore.select(context, driver);
        assertFalse(GpuDriverStore.usedFallback(context));
    }

    @Test
    public void probeCompletionDoesNotClearAnActiveStreamsCrashMarker() throws Exception {
        GpuDriverStore.Driver driver = install(packageFiles());
        GpuDriverStore.select(context, driver);
        assertTrue(GpuDriverStore.beginUse(context, driver));
        assertTrue(GpuDriverStore.beginUse(context, driver));
        GpuDriverStore.endUse(context, driver, false);
        assertEquals(driver.id, context.getSharedPreferences("gpu-drivers", Context.MODE_PRIVATE).getString("active", ""));
        assertThrows(IOException.class, () -> GpuDriverStore.remove(context, driver));
        GpuDriverStore.endUse(context, driver, false);
        assertEquals("", context.getSharedPreferences("gpu-drivers", Context.MODE_PRIVATE).getString("active", ""));
        GpuDriverStore.remove(context, driver);
    }

    private GpuDriverStore.Driver install(Map<String, byte[]> files) throws IOException {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        try (ZipOutputStream zip = new ZipOutputStream(bytes)) {
            for (Map.Entry<String, byte[]> entry : files.entrySet()) {
                zip.putNextEntry(new ZipEntry(entry.getKey()));
                zip.write(entry.getValue());
                zip.closeEntry();
            }
        }
        return GpuDriverStore.importDriver(context, new ByteArrayInputStream(bytes.toByteArray()));
    }

    private static Map<String, byte[]> packageFiles() {
        Map<String, byte[]> files = new LinkedHashMap<>();
        files.put("meta.json", METADATA.getBytes(StandardCharsets.UTF_8));
        files.put("libvulkan_freedreno.so", elf());
        return files;
    }

    private static byte[] elf() {
        byte[] bytes = new byte[64];
        bytes[0] = 0x7f;
        bytes[1] = 'E'; bytes[2] = 'L'; bytes[3] = 'F';
        bytes[4] = 2; bytes[5] = 1; bytes[6] = 1;
        bytes[16] = 3; bytes[18] = (byte) 183;
        return bytes;
    }
}
