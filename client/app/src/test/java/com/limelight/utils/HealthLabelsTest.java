package com.limelight.utils;

import org.junit.Test;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import static org.junit.Assert.assertTrue;

/**
 * Each stream doctor finding starts with its own label in every locale. A label inside another
 * (Russian "Рывки" in "Повторяющиеся рывки") reads as the same finding twice.
 */
public class HealthLabelsTest {
    private static final String[] FINDINGS = {"health_network", "health_decoder", "health_host",
            "health_presentation", "health_heat", "health_periodic", "health_bursts"};

    @Test public void everyFindingHasItsOwnLabelInEveryLocale() throws Exception {
        File[] dirs = new File("src/main/res").listFiles((dir, name) -> new File(dir, name + "/strings_health.xml").isFile());
        assertTrue("strings_health.xml in the default and 14 locales", dirs != null && dirs.length >= 15);
        List<String> problems = new ArrayList<>();
        for (File dir : dirs) {
            String xml = new String(Files.readAllBytes(new File(dir, "strings_health.xml").toPath()), StandardCharsets.UTF_8);
            Map<String, String> labels = new LinkedHashMap<>();
            for (String finding : FINDINGS) {
                Matcher m = Pattern.compile("<string name=\"" + finding + "\">([^:：<]*)[:：]").matcher(xml);
                if (m.find()) {
                    labels.put(finding, m.group(1).trim().toLowerCase(Locale.ROOT));
                } else {
                    problems.add(dir.getName() + ": " + finding + " has no label");
                }
            }
            for (Map.Entry<String, String> a : labels.entrySet()) {
                for (Map.Entry<String, String> b : labels.entrySet()) {
                    if (a != b && b.getValue().contains(a.getValue())) {
                        problems.add(dir.getName() + ": " + a.getKey() + " \"" + a.getValue() + "\" is inside "
                                + b.getKey() + " \"" + b.getValue() + "\"");
                    }
                }
            }
        }
        assertTrue(String.join("\n", problems), problems.isEmpty());
    }
}
