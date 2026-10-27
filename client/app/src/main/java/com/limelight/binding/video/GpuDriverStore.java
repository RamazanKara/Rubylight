package com.limelight.binding.video;

import android.annotation.SuppressLint;
import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.os.Process;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.zip.ZipEntry;
import java.util.zip.ZipInputStream;

public final class GpuDriverStore {
    static final long MAX_ARCHIVE_BYTES = 128L * 1024 * 1024;
    static final long MAX_UNPACKED_BYTES = 256L * 1024 * 1024;
    private static final int MAX_FILES = 64;
    private static final int MAX_DRIVERS = 8;
    private static final Map<String, Integer> active = new HashMap<>();

    public static final class Driver {
        public final String id, name, description, libraryName;
        public final File directory;

        private Driver(File directory, JSONObject metadata) throws JSONException, IOException {
            this.directory = directory;
            id = directory.getName();
            if (integerField(metadata, "schemaVersion") != 1) throw new IOException("Unsupported driver package schema");
            int minApi = integerField(metadata, "minApi");
            if (minApi < 21 || minApi > Build.VERSION.SDK_INT) throw new IOException("Driver requires a different Android version");
            name = field(metadata, "name", 120);
            field(metadata, "author", 120);
            field(metadata, "packageVersion", 120);
            field(metadata, "driverVersion", 120);
            String vendor = field(metadata, "vendor", 120);
            if (!vendor.equals("Mesa") && !vendor.equals("Qualcomm")) throw new IOException("Driver is not for Adreno");
            libraryName = field(metadata, "libraryName", 128);
            if (!safeName(libraryName) || !libraryName.endsWith(".so")) throw new IOException("Invalid driver library name");
            description = metadata.optString("description", "");
            if (description.length() > 4096) throw new IOException("Driver description is too long");
        }
    }

    private GpuDriverStore() { }

    public static boolean isSupported() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && Process.is64Bit() &&
                Build.SUPPORTED_ABIS.length != 0 && "arm64-v8a".equals(Build.SUPPORTED_ABIS[0]) &&
                PyroWaveDecoderRenderer.hasAdrenoGpu();
    }

    private static SharedPreferences preferences(Context context) {
        return context.getSharedPreferences("gpu-drivers", Context.MODE_PRIVATE);
    }

    private static File root(Context context) {
        return new File(context.getNoBackupFilesDir(), "gpu-drivers");
    }

    private static String field(JSONObject metadata, String key, int limit) throws JSONException, IOException {
        Object value = metadata.get(key);
        if (!(value instanceof String) || ((String) value).trim().isEmpty() || ((String) value).length() > limit) {
            throw new IOException("Invalid driver metadata: " + key);
        }
        return (String) value;
    }

    private static boolean safeName(String name) {
        return name.matches("[A-Za-z0-9][A-Za-z0-9._-]{0,127}") && !name.contains("..");
    }

    private static int integerField(JSONObject metadata, String key) throws JSONException, IOException {
        Object value = metadata.get(key);
        if (!(value instanceof Number) || ((Number) value).doubleValue() != ((Number) value).intValue()) {
            throw new IOException("Invalid driver metadata: " + key);
        }
        return ((Number) value).intValue();
    }

    private static Driver read(File directory) throws IOException {
        File metadata = new File(directory, "meta.json");
        if (metadata.length() > 16384) throw new IOException("Driver metadata is too large");
        try (InputStream input = new FileInputStream(metadata)) {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            byte[] buffer = new byte[4096];
            int size;
            while ((size = input.read(buffer)) != -1) {
                if (bytes.size() + size > 16384) throw new IOException("Driver metadata is too large");
                bytes.write(buffer, 0, size);
            }
            Driver driver = new Driver(directory, new JSONObject(bytes.toString(StandardCharsets.UTF_8.name())));
            validateElf(new File(directory, driver.libraryName));
            return driver;
        } catch (JSONException e) {
            throw new IOException("Invalid driver metadata", e);
        }
    }

    static void validateElf(File library) throws IOException {
        byte[] header = new byte[64];
        try (InputStream input = new FileInputStream(library)) {
            int offset = 0;
            while (offset < header.length) {
                int size = input.read(header, offset, header.length - offset);
                if (size < 0) throw new IOException("Driver library is truncated");
                offset += size;
            }
        }
        if (header[0] != 0x7f || header[1] != 'E' || header[2] != 'L' || header[3] != 'F' ||
                header[4] != 2 || header[5] != 1 || header[6] != 1 ||
                header[16] != 3 || header[17] != 0 || (header[18] & 0xff) != 183 || header[19] != 0) {
            throw new IOException("Driver libraries must be arm64 Android ELF shared libraries");
        }
    }

    public static synchronized List<Driver> list(Context context) {
        List<Driver> drivers = new ArrayList<>();
        File[] directories = root(context).listFiles();
        if (directories != null) {
            for (File directory : directories) {
                if (!directory.isDirectory() || !directory.getName().matches("[0-9a-f-]{36}")) continue;
                try {
                    drivers.add(read(directory));
                } catch (IOException ignored) {
                    // Incomplete imports are never offered to the loader.
                }
            }
        }
        drivers.sort(Comparator.comparing(driver -> driver.name));
        return drivers;
    }

    @SuppressLint("ApplySharedPref")
    public static synchronized Driver selected(Context context) {
        SharedPreferences prefs = preferences(context);
        String selected = prefs.getString("selected", "");
        String interrupted = prefs.getString("active", "");
        if (!interrupted.isEmpty() && !active.containsKey(interrupted)) {
            SharedPreferences.Editor edit = prefs.edit().remove("active").putBoolean("fallback", true);
            if (selected.equals(interrupted)) {
                edit.remove("selected");
                selected = "";
            }
            edit.commit();
        }
        if (selected.isEmpty()) return null;
        for (Driver driver : list(context)) if (driver.id.equals(selected)) return driver;
        prefs.edit().remove("selected").apply();
        return null;
    }

    @SuppressLint("ApplySharedPref")
    public static synchronized void select(Context context, Driver driver) {
        preferences(context).edit().putString("selected", driver == null ? "" : driver.id)
                .putBoolean("fallback", false).commit();
    }

    public static boolean usedFallback(Context context) {
        return preferences(context).getBoolean("fallback", false);
    }

    @SuppressLint("ApplySharedPref")
    static synchronized boolean beginUse(Context context, Driver driver) {
        if (!preferences(context).edit().putString("active", driver.id).putBoolean("fallback", false).commit()) {
            return false;
        }
        active.put(driver.id, active.getOrDefault(driver.id, 0) + 1);
        return true;
    }

    @SuppressLint("ApplySharedPref")
    static synchronized void endUse(Context context, Driver driver, boolean failed) {
        int remaining = active.getOrDefault(driver.id, 1) - 1;
        SharedPreferences.Editor edit = preferences(context).edit();
        if (remaining == 0) {
            active.remove(driver.id);
            if (driver.id.equals(preferences(context).getString("active", ""))) edit.remove("active");
        } else {
            active.put(driver.id, remaining);
        }
        if (failed) edit.putBoolean("fallback", true);
        edit.commit();
    }

    public static Driver importDriver(Context context, InputStream source) throws IOException {
        if (list(context).size() >= MAX_DRIVERS) throw new IOException("Remove an unused driver before importing another");
        File base = root(context);
        if (!base.isDirectory() && !base.mkdirs()) throw new IOException("Cannot create driver storage");
        File archive = File.createTempFile("import-", ".zip", base);
        File staging = new File(base, "import-" + UUID.randomUUID());
        try {
            if (!staging.mkdir()) throw new IOException("Cannot create driver import directory");
            try (FileOutputStream output = new FileOutputStream(archive)) {
                copy(source, output, MAX_ARCHIVE_BYTES);
            }
            Set<String> names = new HashSet<>();
            long unpacked = 0;
            try (ZipInputStream zip = new ZipInputStream(new FileInputStream(archive))) {
                ZipEntry entry;
                while ((entry = zip.getNextEntry()) != null) {
                    String name = entry.getName();
                    if (entry.isDirectory() || !safeName(name) || !names.add(name) || names.size() > MAX_FILES) {
                        throw new IOException("Driver ZIP must contain unique files without folders or paths");
                    }
                    File file = new File(staging, name);
                    try (FileOutputStream output = new FileOutputStream(file)) {
                        long limit = name.equals("meta.json") ? Math.min(16384, MAX_UNPACKED_BYTES - unpacked) : MAX_UNPACKED_BYTES - unpacked;
                        unpacked += copy(zip, output, limit);
                    }
                    if (name.endsWith(".so")) validateElf(file);
                }
            }
            read(staging);
            File[] files = staging.listFiles();
            if (files != null) {
                for (File file : files) {
                    if (!file.setWritable(false, false)) throw new IOException("Cannot protect the imported driver files");
                }
            }
            File installed = new File(base, UUID.randomUUID().toString());
            if (!staging.renameTo(installed)) throw new IOException("Cannot install the driver package");
            return read(installed);
        } finally {
            deleteInside(base, staging);
            if (!archive.delete() && archive.exists()) archive.deleteOnExit();
        }
    }

    private static long copy(InputStream source, FileOutputStream output, long limit) throws IOException {
        byte[] buffer = new byte[32768];
        long total = 0;
        int size;
        while ((size = source.read(buffer)) != -1) {
            total += size;
            if (total > limit) throw new IOException("Driver package exceeds the size limit");
            output.write(buffer, 0, size);
        }
        return total;
    }

    public static synchronized void remove(Context context, Driver driver) throws IOException {
        if (active.containsKey(driver.id)) throw new IOException("Stop the stream before removing its driver");
        if (driver.id.equals(preferences(context).getString("selected", ""))) select(context, null);
        deleteInside(root(context), driver.directory);
    }

    private static void deleteInside(File base, File file) throws IOException {
        if (!file.exists()) return;
        if (!file.getCanonicalPath().startsWith(base.getCanonicalPath() + File.separator)) {
            throw new IOException("Invalid driver storage path");
        }
        File[] children = file.listFiles();
        if (children != null) for (File child : children) deleteInside(base, child);
        if (!file.delete()) throw new IOException("Cannot remove driver files");
    }
}
