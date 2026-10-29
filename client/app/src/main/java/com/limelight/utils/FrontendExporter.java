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
import com.limelight.grid.assets.CachedAppAssetLoader;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.grid.assets.NetworkAssetLoader;
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
    private CoverSource covers = new CoverSource() {
        @Override
        public File cover(NvApp app) {
            return cachedCover(app);
        }
    };

    interface CoverSource {
        File cover(NvApp app);
    }

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

    /** Fetches covers the app grid never loaded from the PC while exporting. */
    public void downloadMissingCovers(final String uniqueId) {
        covers = new CoverSource() {
            @Override
            public File cover(NvApp app) {
                File cached = cachedCover(app);
                if (cached.isFile()) return cached;
                CachedAppAssetLoader.LoaderTuple tuple = new CachedAppAssetLoader.LoaderTuple(computer, app);
                try (InputStream in = new NetworkAssetLoader(context, uniqueId).getBitmapStream(tuple)) {
                    if (in != null) new DiskAssetLoader(context).populateCacheWithStream(tuple, in);
                } catch (IOException | RuntimeException e) {
                    // An offline PC only costs this export the cover; a later export fetches it.
                    LimeLog.warning("Unable to fetch cover for " + app.getAppName() + ": " + e.getMessage());
                }
                return cached;
            }
        };
    }

    private File cachedCover(NvApp app) {
        return CacheHelper.openPath(false, context.getCacheDir(), "boxart",
                computer.uuid, app.getAssetCacheKey() + ".png");
    }

    /** Returns the number of entries written. Either tree may be null except the ROMs one. */
    public int export(List<NvApp> apps, Uri romsTree, Uri esdeTree) throws IOException {
        return export(apps, romsTree, esdeTree, new HashMap<String, String>());
    }

    /**
     * Makes the folder match the PC's games: entries this export wrote for this PC's games that
     * are gone or hidden are removed with their covers. {@code ownCovers} records the covers
     * Nightjar wrote (file name to asset key, size and time); only a cover still exactly as
     * written is replaced when the art changes or removed, and the record is updated in place.
     * Covers the user placed or changed are never touched.
     */
    public int export(List<NvApp> apps, Uri romsTree, Uri esdeTree, Map<String, String> ownCovers)
            throws IOException {
        Uri romsRoot = treeRoot(romsTree);
        Uri systemDir = directory(romsTree, romsRoot, FrontendEntry.ES_SYSTEM_NAME);
        Map<String, Uri> existing = new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
        existing.putAll(listChildren(romsTree, systemDir));
        Map<String, Map<String, String>> entries = new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
        Set<String> exported = new HashSet<>();
        for (Map.Entry<String, Uri> child : existing.entrySet()) {
            String name = child.getKey();
            if (name.length() >= FrontendEntry.EXTENSION.length() && name.regionMatches(true,
                    name.length() - FrontendEntry.EXTENSION.length(), FrontendEntry.EXTENSION, 0,
                    FrontendEntry.EXTENSION.length())) {
                String raw = readEntry(child.getValue());
                Map<String, String> values = parseEntry(raw);
                if (values != null) {
                    entries.put(name, values);
                    if (FrontendEntry.writtenByExport(raw, values)) exported.add(name);
                }
            }
        }

        Uri coversDir = null;
        Map<String, CoverFile> existingCovers = new HashMap<>();
        if (esdeTree != null) {
            registerEsDeSystem(esdeTree);
            Uri esdeRoot = treeRoot(esdeTree);
            Uri media = directory(esdeTree, esdeRoot, "downloaded_media");
            Uri systemMedia = directory(esdeTree, media, FrontendEntry.ES_SYSTEM_NAME);
            coversDir = directory(esdeTree, systemMedia, "covers");
            existingCovers = listCovers(esdeTree, coversDir);
            // A cover deleted, replaced or edited since we wrote it is no longer ours.
            for (java.util.Iterator<Map.Entry<String, String>> own = ownCovers.entrySet().iterator(); own.hasNext();) {
                Map.Entry<String, String> record = own.next();
                if (!stillAsWritten(record.getValue(), existingCovers.get(record.getKey()))) own.remove();
            }
        }

        // Gone first, so a new game may take a removed game's file name.
        for (String gone : staleEntries(entries, exported, apps)) {
            delete(existing.remove(gone));
            entries.remove(gone);
            String cover = baseName(gone) + ".png";
            if (coversDir != null && ownCovers.remove(cover) != null) {
                delete(existingCovers.remove(cover).uri);
            }
        }

        int written = 0;
        for (NvApp app : apps) {
            String fileName = entryFileName(app, existing, entries);
            String base = baseName(fileName);
            String contents = FrontendEntry.serializeExport(computer.uuid, computer.name,
                    app.getAppUuid(), app.getAppName(), app.getAppId());
            Map<String, String> values = FrontendEntry.parse(new StringReader(contents));
            Uri entry = existing.get(fileName);
            if (entry == null) {
                entry = createText(systemDir, ENTRY_MIME, fileName, contents);
                existing.put(fileName, entry);
            } else if (!values.equals(entries.get(fileName))) {
                writeText(entry, contents);
            }
            entries.put(fileName, values);
            written++;

            if (coversDir != null) {
                String name = base + ".png";
                String key = app.getAssetCacheKey();
                CoverFile current = existingCovers.get(name);
                String record = ownCovers.get(name);
                if (current == null || (record != null && !key.equals(record.split("\t", -1)[0]))) {
                    Uri cover = copyCover(app, coversDir, name, current == null ? null : current.uri);
                    if (cover != null) {
                        CoverFile copied = stat(cover);
                        existingCovers.put(name, copied);
                        ownCovers.put(name, key + "\t" + copied.size + "\t" + copied.modified);
                    }
                }
            }
        }
        return written;
    }

    /** A cover in the ES-DE folder, with what its provider reports (-1 when it reports nothing). */
    private static final class CoverFile {
        final Uri uri;
        final long size;
        final long modified;

        CoverFile(Uri uri, long size, long modified) {
            this.uri = uri;
            this.size = size;
            this.modified = modified;
        }
    }

    /** Whether a cover is still byte-for-byte the file we wrote, as far as size and time tell. */
    static boolean stillAsWritten(String record, CoverFile cover) {
        if (cover == null) return false;
        String[] parts = record.split("\t", -1);
        if (parts.length != 3) return false;
        try {
            long size = Long.parseLong(parts[1]);
            long modified = Long.parseLong(parts[2]);
            // Without either value nothing proves the file is ours; leave it to the user.
            return (size >= 0 || modified > 0) && size == cover.size && modified == cover.modified;
        } catch (NumberFormatException e) {
            return false;
        }
    }

    private static final String[] COVER_COLUMNS = {Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME,
            Document.COLUMN_SIZE, Document.COLUMN_LAST_MODIFIED};

    private Map<String, CoverFile> listCovers(Uri tree, Uri dir) throws IOException {
        Map<String, CoverFile> covers = new HashMap<>();
        Uri query = DocumentsContract.buildChildDocumentsUriUsingTree(tree, DocumentsContract.getDocumentId(dir));
        try (Cursor cursor = resolver.query(query, COVER_COLUMNS, null, null, null)) {
            if (cursor == null) {
                throw new IOException("Unable to list folder");
            }
            while (cursor.moveToNext()) {
                covers.put(cursor.getString(1), new CoverFile(DocumentsContract.buildDocumentUriUsingTree(tree,
                        cursor.getString(0)), number(cursor, 2), number(cursor, 3)));
            }
        }
        return covers;
    }

    private CoverFile stat(Uri cover) {
        try (Cursor cursor = resolver.query(cover, COVER_COLUMNS, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                return new CoverFile(cover, number(cursor, 2), number(cursor, 3));
            }
        } catch (RuntimeException e) {
            LimeLog.warning("Unable to read the written cover's details: " + e.getMessage());
        }
        // Unknown details: the record can never match, so the cover is left to the user.
        return new CoverFile(cover, -1, -1);
    }

    private static long number(Cursor cursor, int column) {
        return cursor.isNull(column) ? -1 : cursor.getLong(column);
    }

    /** This PC's entries the export wrote that match none of its current games: they are stale. */
    private List<String> staleEntries(Map<String, Map<String, String>> entries, Set<String> exported,
                                      List<NvApp> apps) {
        List<String> stale = new ArrayList<>();
        for (Map.Entry<String, Map<String, String>> entry : entries.entrySet()) {
            if (!exported.contains(entry.getKey())) continue;
            if (!computer.uuid.equalsIgnoreCase(entry.getValue().get(FrontendEntry.KEY_HOST_UUID))) continue;
            boolean current = false;
            for (NvApp app : apps) {
                if (FrontendEntry.matchesApp(entry.getValue(), computer.uuid, app.getAppUuid(), app.getAppId())) {
                    current = true;
                    break;
                }
            }
            if (!current) stale.add(entry.getKey());
        }
        return stale;
    }

    private static String baseName(String entryFileName) {
        return entryFileName.substring(0, entryFileName.length() - FrontendEntry.EXTENSION.length());
    }

    private void delete(Uri document) throws IOException {
        if (document == null) return;
        try {
            if (!DocumentsContract.deleteDocument(resolver, document)) {
                throw new IOException("Unable to remove " + document);
            }
        } catch (RuntimeException e) {
            throw new IOException("Unable to remove " + document, e);
        }
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

    /** The entry's text, or null when unreadable or too large to be an entry. */
    private String readEntry(Uri entry) {
        try (InputStream in = resolver.openInputStream(entry)) {
            if (in == null) {
                return null;
            }
            InputStreamReader reader = new InputStreamReader(in, StandardCharsets.UTF_8);
            StringBuilder text = new StringBuilder();
            char[] buffer = new char[4096];
            int count;
            while ((count = reader.read(buffer)) != -1) {
                text.append(buffer, 0, count);
                if (text.length() > FrontendEntry.MAX_CHARS) return null;
            }
            return text.toString();
        } catch (IOException | SecurityException e) {
            return null;
        }
    }

    private static Map<String, String> parseEntry(String raw) {
        if (raw == null) return null;
        try {
            return FrontendEntry.parse(new StringReader(raw));
        } catch (IOException e) {
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

    /** Writes the cover into {@code existing} when given (one of ours whose art changed), else a new file. */
    private Uri copyCover(NvApp app, Uri coversDir, String name, Uri existing) {
        File cached = covers.cover(app);
        if (cached == null || !cached.isFile()) {
            return null;
        }
        Uri created = null;
        Uri target = existing;
        try (InputStream in = new FileInputStream(cached)) {
            if (target == null) target = created = create(coversDir, "image/png", name);
            try (OutputStream out = resolver.openOutputStream(target, "wt")) {
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
        return target;
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
