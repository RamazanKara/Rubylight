package com.limelight.update;

import java.math.BigInteger;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/** A release version such as v0.10.0 or 1.2.3-rc.1; build metadata after + is ignored. */
public final class SemVer implements Comparable<SemVer> {
    private static final Pattern FORMAT =
            Pattern.compile("^[vV]?(\\d+)(?:\\.(\\d+))?(?:\\.(\\d+))?(?:-([0-9A-Za-z.-]+))?(?:\\+[0-9A-Za-z.-]+)?$");

    public final int major;
    public final int minor;
    public final int patch;
    /** The part after the first hyphen, or null for a final release. */
    public final String prerelease;

    private SemVer(int major, int minor, int patch, String prerelease) {
        this.major = major;
        this.minor = minor;
        this.patch = patch;
        this.prerelease = prerelease;
    }

    /** Returns null when the text is not a version. A leading v is ignored. */
    public static SemVer parse(String text) {
        if (text == null) {
            return null;
        }
        Matcher m = FORMAT.matcher(text.trim());
        if (!m.matches()) {
            return null;
        }
        try {
            return new SemVer(Integer.parseInt(m.group(1)),
                    m.group(2) == null ? 0 : Integer.parseInt(m.group(2)),
                    m.group(3) == null ? 0 : Integer.parseInt(m.group(3)),
                    m.group(4));
        } catch (NumberFormatException e) {
            return null;
        }
    }

    public boolean isPrerelease() {
        return prerelease != null;
    }

    /** Whether the release tag names a newer version than the running one. Unparseable versions are never newer. */
    public static boolean isNewer(String candidate, String current) {
        SemVer a = parse(candidate);
        SemVer b = parse(current);
        return a != null && b != null && a.compareTo(b) > 0;
    }

    /** The version without a leading v, the way file names and the skipped version are written. */
    public static String normalize(String text) {
        SemVer v = parse(text);
        return v == null ? (text == null ? "" : text.trim()) : v.toString();
    }

    @Override
    public int compareTo(SemVer other) {
        if (major != other.major) {
            return Integer.compare(major, other.major);
        }
        if (minor != other.minor) {
            return Integer.compare(minor, other.minor);
        }
        if (patch != other.patch) {
            return Integer.compare(patch, other.patch);
        }
        if (prerelease == null || other.prerelease == null) {
            // A final release is newer than any prerelease of the same number.
            return prerelease == null ? (other.prerelease == null ? 0 : 1) : -1;
        }
        String[] a = prerelease.split("\\.");
        String[] b = other.prerelease.split("\\.");
        for (int i = 0; i < Math.min(a.length, b.length); i++) {
            boolean numA = a[i].matches("\\d+");
            boolean numB = b[i].matches("\\d+");
            int result;
            if (numA && numB) {
                result = new BigInteger(a[i]).compareTo(new BigInteger(b[i]));
            } else if (numA != numB) {
                result = numA ? -1 : 1;
            } else {
                result = a[i].compareTo(b[i]);
            }
            if (result != 0) {
                return result;
            }
        }
        return Integer.compare(a.length, b.length);
    }

    @Override
    public boolean equals(Object o) {
        return o instanceof SemVer && compareTo((SemVer) o) == 0;
    }

    @Override
    public int hashCode() {
        return 31 * (31 * (31 * major + minor) + patch) + (prerelease == null ? 0 : prerelease.hashCode());
    }

    @Override
    public String toString() {
        return major + "." + minor + "." + patch + (prerelease == null ? "" : "-" + prerelease);
    }
}
