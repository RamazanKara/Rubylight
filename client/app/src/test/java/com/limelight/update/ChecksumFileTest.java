package com.limelight.update;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

public class ChecksumFileTest {
    private static final String APK = "454f9e4a7f46253dcfa138208ea392a8e3c43e862659ef559b060f3d8fb1fd80";
    private static final String ROOT = "ca6ef9d3790517db71acb2b0e709e16c2e2c54e9c42222ca527782944bf09f64";

    @Test
    public void reads_the_binary_marker_form() {
        String sums = APK + " *nightjar-0.7.0.apk\n" + ROOT + " *nightjar-0.7.0-root.apk\n";
        assertEquals(APK, ChecksumFile.find(sums, "nightjar-0.7.0.apk"));
        assertEquals(ROOT, ChecksumFile.find(sums, "nightjar-0.7.0-root.apk"));
    }

    @Test
    public void reads_the_text_form_with_two_spaces() {
        String sums = APK + "  nightjar-0.7.0.apk\r\n" + ROOT + "  nightjar-0.7.0-root.apk\r\n";
        assertEquals(APK, ChecksumFile.find(sums, "nightjar-0.7.0.apk"));
        assertEquals(ROOT, ChecksumFile.find(sums, "nightjar-0.7.0-root.apk"));
    }

    @Test
    public void matches_the_whole_file_name_and_lowercases_the_digest() {
        String sums = APK.toUpperCase() + "  nightjar-0.7.0.apk.sig\n" + ROOT + "  other-nightjar-0.7.0.apk\n";
        assertNull(ChecksumFile.find(sums, "nightjar-0.7.0.apk"));
        assertEquals(APK, ChecksumFile.find(APK.toUpperCase() + "  nightjar-0.7.0.apk", "nightjar-0.7.0.apk"));
    }

    @Test
    public void accepts_a_path_before_the_name() {
        assertEquals(APK, ChecksumFile.find(APK + "  app/build/outputs/nightjar-0.7.0.apk\n", "nightjar-0.7.0.apk"));
    }

    @Test
    public void ignores_junk_and_reports_a_missing_entry() {
        String sums = "# checksums\n\nnot a hash  nightjar-0.7.0.apk\n" + APK.substring(1) + "  nightjar-0.7.0.apk\n";
        assertNull(ChecksumFile.find(sums, "nightjar-0.7.0.apk"));
        assertNull(ChecksumFile.find("", "nightjar-0.7.0.apk"));
        assertNull(ChecksumFile.find(null, "nightjar-0.7.0.apk"));
    }

    @Test
    public void formats_a_digest_as_lowercase_hex() {
        assertEquals("00ff10", ChecksumFile.hex(new byte[] {0, (byte) 0xff, 0x10}));
    }
}
