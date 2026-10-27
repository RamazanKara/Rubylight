package com.limelight.update;

import java.util.Locale;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/** Reads sha256sum output ("hex  name" or "hex *name") and formats digests. */
public final class ChecksumFile {
    private static final Pattern LINE = Pattern.compile("^([0-9a-fA-F]{64})[ \\t]+\\*?(.+?)\\s*$");

    private ChecksumFile() {
    }

    /** The lowercase hex digest listed for the file name, or null when no line names it. */
    public static String find(String sums, String fileName) {
        if (sums == null || fileName == null) {
            return null;
        }
        for (String line : sums.split("\\r?\\n")) {
            Matcher m = LINE.matcher(line.trim());
            if (!m.matches()) {
                continue;
            }
            String name = m.group(2);
            // A sums file made in another folder names the file with its path.
            name = name.substring(Math.max(name.lastIndexOf('/'), name.lastIndexOf('\\')) + 1);
            if (name.equals(fileName)) {
                return m.group(1).toLowerCase(Locale.ROOT);
            }
        }
        return null;
    }

    public static String hex(byte[] bytes) {
        StringBuilder out = new StringBuilder(bytes.length * 2);
        for (byte b : bytes) {
            out.append(String.format(Locale.ROOT, "%02x", b & 0xff));
        }
        return out.toString();
    }
}
