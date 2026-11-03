package com.limelight.grid.assets;

import java.io.DataInputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;

/**
 * The grey box a host sends from {@code /appasset} for a game with no cover of its own. Hosts answer that request
 * with their {@code box.png} rather than a 404, so the file is recognised by its bytes, and the title stands in for it
 * as for a game with no art at all.
 */
public final class PlaceholderArt {
    // SHA-256 of each known box.png, keyed by its length so a real cover is never hashed.
    private static final long[] LENGTHS = {680, 998};
    private static final String[] DIGESTS = {
            // Rubylight (rust/assets/package/box.png) and Sunshine-family hosts since Sunshine 0.21.
            "d9164ebd069b5f735eb8efc557801778498da37f572ef70e3d35604739e6c613",
            // Sunshine 0.20 and earlier.
            "f5992b68a72ef6f74c6e4970d0eb081fd7c51e8b5556c6a3f87dc414fda0473b",
    };

    private PlaceholderArt() {
    }

    public static boolean matches(File file) {
        long length = file.length();
        for (int i = 0; i < LENGTHS.length; i++) {
            if (length == LENGTHS[i]) {
                byte[] data = new byte[(int) length];
                try (DataInputStream in = new DataInputStream(new FileInputStream(file))) {
                    in.readFully(data);
                    return DIGESTS[i].equals(sha256(data));
                } catch (IOException e) {
                    // An unreadable file decodes to no cover anyway.
                    return false;
                }
            }
        }
        return false;
    }

    static String sha256(byte[] data) {
        try {
            StringBuilder hex = new StringBuilder();
            for (byte b : MessageDigest.getInstance("SHA-256").digest(data)) {
                hex.append(String.format((java.util.Locale) null, "%02x", b));
            }
            return hex.toString();
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException(e);
        }
    }
}
