package com.limelight.update;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

public class ReleaseNotesTest {
    @Test
    public void strips_markdown_down_to_plain_text() {
        String notes = "## What's new\r\n\r\n- **Faster** pairing\r\n* See [the docs](https://example.com/docs)\r\n"
                + "> Note `code`\r\n![shot](https://example.com/a.png)\r\n<br>Done";
        assertEquals("What's new\n\n• Faster pairing\n• See the docs\nNote code\n\nDone",
                ReleaseNotes.plain(notes, 1000));
    }

    @Test
    public void trims_and_collapses_blank_lines() {
        assertEquals("a\n\nb", ReleaseNotes.plain("\n\n  a\n\n\n\n\nb  \n\n", 100));
        assertEquals("", ReleaseNotes.plain(null, 100));
        assertEquals("", ReleaseNotes.plain("   ", 100));
    }

    @Test
    public void cuts_long_notes_with_an_ellipsis() {
        String text = ReleaseNotes.plain("x".repeat(50), 10);
        assertEquals(10, text.length());
        assertTrue(text.endsWith("…"));
        assertEquals("short", ReleaseNotes.plain("short", 10));
    }

    @Test
    public void does_not_split_a_surrogate_pair() {
        String text = ReleaseNotes.plain("abcd😀efgh", 6);
        assertTrue(text.endsWith("…"));
        assertEquals("abcd…", text);
    }
}
