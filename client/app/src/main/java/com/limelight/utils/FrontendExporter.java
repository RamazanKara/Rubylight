package com.limelight.utils;

import android.content.ContentResolver;
import android.content.Context;
import android.database.Cursor;
import android.net.Uri;
import android.provider.DocumentsContract;
import android.provider.DocumentsContract.Document;

import com.limelight.AppView;
import com.limelight.BuildConfig;
import com.limelight.LimeLog;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;

import org.xmlpull.v1.XmlPullParserException;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.io.StringReader;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;

/**
 * Writes one game entry file per host app into a frontend's ROMs folder and, when given the
 * ES-DE folder, registers our system there and copies cached cover art.
 *
 * Both folders come from the Storage Access Framework, so no storage permission is needed.
 */
public final class FrontendExporter {
    private static final String ENTRY_MIME = "application/octet-stream";

    private final Context context;
    private final ContentResolver resolver;
    private final ComputerDetails computer;

    public FrontendExporter(Context context, ComputerDetails computer) {
        this.context = context.getApplicationContext();
        this.resolver = context.getContentResolver();
        this.computer = computer;
    }

    /** Apps from the cached host list, without the ones hidden in the app grid. */
    public List<NvApp> loadApps() {
        List<NvApp> apps = new ArrayList<>();
        try {
            String raw = CacheHelper.readInputStreamToString(
                    CacheHelper.openCacheFileForInput(context.getCacheDir(), "applist", computer.uuid));
            if (raw.isEmpty()) {
                return apps;
            }
            Set<String> hidden = context.getSharedPreferences(AppView.HIDDEN_APPS_PREF_FILENAME, Context.MODE_PRIVATE)
                    .getStringSet(computer.uuid, new HashSet<String>());
            for (NvApp app : NvHTTP.getAppListByReader(new StringReader(raw), computer.caps)) {
                if (!hidden.contains(Integer.toString(app.getAppId()))) {
                    apps.add(app);
                }
            }
        } catch (IOException | XmlPullParserException e) {
            LimeLog.info("No cached app list for frontend export: " + e.getMessage());
        }
        return apps;
    }

    /** Returns the number of entries written. Either tree may be null except the ROMs one. */
    public int export(List<NvApp> apps, Uri romsTree, Uri esdeTree) throws IOException {
        Uri romsRoot = treeRoot(romsTree);
        Uri systemDir = directory(romsTree, romsRoot, FrontendEntry.ES_SYSTEM_NAME);
        Map<String, Uri> existing = new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
        existing.putAll(listChildren(romsTree, systemDir));
        Map<String, Map<String, String>> entries = new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
        for (Map.Entry<String, Uri> child : existing.entrySet()) {
            String name = child.getKey();
            if (name.length() >= FrontendEntry.EXTENSION.length() && name.regionMatches(true,
                    name.length() - FrontendEntry.EXTENSION.length(), FrontendEntry.EXTENSION, 0,
                    FrontendEntry.EXTENSION.length())) {
                Map<String, String> values = readEntry(child.getValue());
                if (values != null) entries.put(name, values);
            }
        }

        Uri coversDir = null;
        Map<String, Uri> existingCovers = new HashMap<>();
        if (esdeTree != null) {
            registerEsDeSystem(esdeTree);
            Uri esdeRoot = treeRoot(esdeTree);
            Uri media = directory(esdeTree, esdeRoot, "downloaded_media");
            Uri systemMedia = directory(esdeTree, media, FrontendEntry.ES_SYSTEM_NAME);
            coversDir = directory(esdeTree, systemMedia, "covers");
            existingCovers = listChildren(esdeTree, coversDir);
        }

        int written = 0;
        for (NvApp app : apps) {
            String fileName = entryFileName(app, existing, entries);
            String base = fileName.substring(0, fileName.length() - FrontendEntry.EXTENSION.length());
            String contents = FrontendEntry.serialize(computer.uuid, computer.name,
                    app.getAppUuid(), app.getAppName(), app.getAppId());
            Map<String, String> values = FrontendEntry.parse(new StringReader(contents));
            Uri entry = existing.get(fileName);
            if (entry == null) {
                entry = createText(systemDir, ENTRY_MIME, fileName, contents);
                existing.put(fileName, entry);
            } else {
                writeText(entry, contents);
            }
            entries.put(fileName, values);
            written++;

            if (coversDir != null && !existingCovers.containsKey(base + ".png")) {
                Uri cover = copyCover(app, coversDir, base + ".png");
                if (cover != null) existingCovers.put(base + ".png", cover);
            }
        }
        return written;
    }

    /**
     * Preserve an existing game's file even when names collide after sanitizing or truncating.
     */
    private String entryFileName(NvApp app, Map<String, Uri> existing,
                                Map<String, Map<String, String>> entries) {
        for (Map.Entry<String, Map<String, String>> entry : entries.entrySet()) {
            if (FrontendEntry.matchesApp(entry.getValue(), computer.uuid, app.getAppUuid(), app.getAppId())) {
                return entry.getKey();
            }
        }
        String base = FrontendEntry.fileBaseName(app.getAppName());
        String hostBase = FrontendEntry.fileBaseName(app.getAppName() + " (" + computer.name + ")");
        for (int suffix = 1; ; suffix++) {
            if (!existing.containsKey(base + FrontendEntry.EXTENSION)) {
                return base + FrontendEntry.EXTENSION;
            }
            base = suffix == 1 ? hostBase : hostBase + " (" + suffix + ")";
        }
    }

    private Map<String, String> readEntry(Uri entry) {
        try (InputStream in = resolver.openInputStream(entry)) {
            if (in == null) {
                return null;
            }
            return FrontendEntry.parse(new InputStreamReader(in, StandardCharsets.UTF_8));
        } catch (IOException | SecurityException e) {
            return null;
        }
    }

    private void registerEsDeSystem(Uri esdeTree) throws IOException {
        Uri custom = directory(esdeTree, treeRoot(esdeTree), "custom_systems");
        Map<String, Uri> files = listChildren(esdeTree, custom);
        mergeXml(custom, files, "es_systems.xml", new XmlMerge() {
            @Override
            public String merge(String existing) {
                return FrontendEntry.mergeEsSystems(existing, BuildConfig.BRAND_NAME);
            }
        });
        mergeXml(custom, files, "es_find_rules.xml", new XmlMerge() {
            @Override
            public String merge(String existing) {
                return FrontendEntry.mergeEsFindRules(existing, context.getPackageName());
            }
        });
    }

    private interface XmlMerge {
        String merge(String existing);
    }

    private void mergeXml(Uri dir, Map<String, Uri> files, String name, XmlMerge merge) throws IOException {
        Uri file = files.get(name);
        String existing = null;
        if (file != null) {
            try (InputStream in = resolver.openInputStream(file)) {
                if (in == null) {
                    throw new IOException("Unable to read " + name);
                }
                existing = CacheHelper.readInputStreamToString(in);
            }
        }
        String contents = merge.merge(existing);
        if (file == null) {
            createText(dir, "text/xml", name, contents);
        } else {
            writeText(file, contents);
        }
    }

    private Uri copyCover(NvApp app, Uri coversDir, String name) {
        File cached = CacheHelper.openPath(false, context.getCacheDir(), "boxart",
                computer.uuid, app.getAssetCacheKey() + ".png");
        if (!cached.isFile()) {
            return null;
        }
        Uri created = null;
        try (InputStream in = new FileInputStream(cached)) {
            created = create(coversDir, "image/png", name);
            try (OutputStream out = resolver.openOutputStream(created, "wt")) {
                if (out == null) throw new IOException("Unable to write cover");
                byte[] buffer = new byte[16384];
                int count;
                while ((count = in.read(buffer)) != -1) {
                    out.write(buffer, 0, count);
                }
            }
        } catch (IOException | SecurityException e) {
            if (created != null) deleteIncomplete(created);
            LimeLog.warning("Unable to copy cover for " + app.getAppName() + ": " + e.getMessage());
            return null;
        }
        return created;
    }

    private Uri createText(Uri parent, String mime, String name, String contents) throws IOException {
        Uri created = create(parent, mime, name);
        try {
            writeText(created, contents);
            return created;
        } catch (IOException | RuntimeException failure) {
            deleteIncomplete(created);
            throw failure;
        }
    }

    private void deleteIncomplete(Uri document) {
        try {
            if (!DocumentsContract.deleteDocument(resolver, document)) {
                LimeLog.warning("Unable to remove incomplete frontend document");
            }
        } catch (IOException | RuntimeException failure) {
            LimeLog.warning("Unable to remove incomplete frontend document: " + failure.getMessage());
        }
    }

    private void writeText(Uri file, String text) throws IOException {
        try (OutputStream out = resolver.openOutputStream(file, "wt")) {
            if (out == null) {
                throw new IOException("Unable to write " + file);
            }
            out.write(text.getBytes(StandardCharsets.UTF_8));
        }
    }

    private static Uri treeRoot(Uri tree) {
        return DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree));
    }

    private Uri directory(Uri tree, Uri parent, String name) throws IOException {
        Uri dir = listChildren(tree, parent).get(name);
        return dir != null ? dir : create(parent, Document.MIME_TYPE_DIR, name);
    }

    private Uri create(Uri parent, String mime, String name) throws IOException {
        Uri created = DocumentsContract.createDocument(resolver, parent, mime, name);
        if (created == null) {
            throw new IOException("Unable to create " + name);
        }
        return created;
    }

    private Map<String, Uri> listChildren(Uri tree, Uri parent) throws IOException {
        Map<String, Uri> children = new HashMap<>();
        Uri query = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getDocumentId(parent));
        try (Cursor cursor = resolver.query(query, new String[]{
                Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME}, null, null, null)) {
            if (cursor == null) {
                throw new IOException("Unable to list folder");
            }
            while (cursor.moveToNext()) {
                children.put(cursor.getString(1), DocumentsContract.buildDocumentUriUsingTree(tree, cursor.getString(0)));
            }
        }
        return children;
    }
}
