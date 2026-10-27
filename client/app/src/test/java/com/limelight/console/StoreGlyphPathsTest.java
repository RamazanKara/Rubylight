package com.limelight.console;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * The store glyphs come from Simple Icons, whose paths pack arc flags ("0 00.02.433"). Android's PathParser reads
 * those as fewer floats than the arc needs and throws when the card binds, which Robolectric never sees: this
 * splits every path the way PathParser does and checks each command gets whole sets of numbers.
 */
public class StoreGlyphPathsTest {
    private static final Pattern PATH_DATA = Pattern.compile("pathData=\"([^\"]*)\"");

    @Test
    public void everyStoreGlyphParsesAsAndroidReadsIt() throws IOException {
        File[] glyphs = new File("src/main/res/drawable").listFiles((dir, name) -> name.startsWith("ic_store_"));
        assertTrue("the store glyphs are found", glyphs != null && glyphs.length > 0);
        for (File glyph : glyphs) {
            Matcher path = PATH_DATA.matcher(new String(Files.readAllBytes(glyph.toPath()), StandardCharsets.UTF_8));
            assertTrue(glyph.getName() + " has a path", path.find());
            do {
                check(glyph.getName(), path.group(1));
            } while (path.find());
        }
    }

    @Test
    public void packedArcFlagsAreCaught() {
        assertEquals(5, floats("4.262 4.262 0 00.02.433").size());
        assertEquals(7, floats("4.262 4.262 0 0 0 .02 .433").size());
    }

    private static void check(String name, String data) {
        int start = 0;
        for (int i = 1; i <= data.length(); i++) {
            if (i == data.length() || isCommand(data.charAt(i))) {
                char command = Character.toLowerCase(data.charAt(start));
                int count = floats(data.substring(start + 1, i)).size();
                int arguments = arguments(command);
                assertTrue(name + ": '" + data.charAt(start) + "' at " + start + " has " + count + " numbers",
                        arguments == 0 ? count == 0 : count > 0 && count % arguments == 0);
                start = i;
            }
        }
    }

    private static boolean isCommand(char c) {
        return Character.isLetter(c) && c != 'e' && c != 'E';
    }

    private static int arguments(char command) {
        switch (command) {
            case 'z': return 0;
            case 'h': case 'v': return 1;
            case 'm': case 'l': case 't': return 2;
            case 's': case 'q': return 4;
            case 'c': return 6;
            case 'a': return 7;
            default: throw new AssertionError("unknown path command " + command);
        }
    }

    /** android.util.PathParser's float split: a number ends at a space, a comma, a sign not after e, or a second dot. */
    private static List<String> floats(String s) {
        List<String> out = new ArrayList<>();
        int i = 0;
        while (i < s.length()) {
            char c = s.charAt(i);
            if (c == ' ' || c == ',') {
                i++;
                continue;
            }
            int end = i;
            boolean dot = false;
            boolean exponent = false;
            for (int j = i; j < s.length(); j++) {
                char d = s.charAt(j);
                boolean last = exponent;
                exponent = false;
                if (d == ' ' || d == ',') break;
                if (d == '-' && j != i && !last) break;
                if (d == '.') {
                    if (dot) break;
                    dot = true;
                }
                if (d == 'e' || d == 'E') exponent = true;
                end = j + 1;
            }
            out.add(s.substring(i, end));
            i = end;
        }
        return out;
    }
}
