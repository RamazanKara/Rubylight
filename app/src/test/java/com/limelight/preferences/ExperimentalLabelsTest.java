package com.limelight.preferences;

import org.junit.Test;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

/**
 * No string the app shows says Experimental, in any locale (ux/labels.md): features ship plainly and
 * their keys stay. Resource names such as title_experimental_chipset_tuning are identifiers, not text.
 */
public class ExperimentalLabelsTest {
    private static final Pattern WORD = Pattern.compile("experimental|experimentell|expérimental|sperimental|"
            + "試験運用|試験的|実験的|실험적|eksperymentaln|экспериментал|deneysel|експериментал|thử nghiệm|实验性|實驗性",
            Pattern.CASE_INSENSITIVE | Pattern.UNICODE_CASE);
    private static final Pattern VALUE = Pattern.compile("<(string|item)\\b[^>]*>(.*?)</\\1>", Pattern.DOTALL);

    @Test
    public void noShownStringSaysExperimentalInAnyLocale() throws Exception {
        File[] dirs = new File("src/main/res").listFiles((dir, name) -> name.startsWith("values"));
        assertTrue("resource folders found", dirs != null && dirs.length >= 15);
        List<String> found = new ArrayList<>();
        int strings = 0;
        for (File dir : dirs) {
            File[] files = dir.listFiles((d, name) -> name.endsWith(".xml"));
            for (File file : files) {
                Matcher m = VALUE.matcher(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
                while (m.find()) {
                    strings++;
                    if (WORD.matcher(m.group(2)).find()) {
                        found.add(dir.getName() + "/" + file.getName() + ": " + m.group(2));
                    }
                }
            }
        }
        assertTrue("strings read", strings > 1000);
        assertEquals(new ArrayList<String>(), found);
    }
}
