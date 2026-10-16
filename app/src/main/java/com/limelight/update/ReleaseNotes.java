package com.limelight.update;

import java.util.regex.Pattern;

/** Turns a release's Markdown body into short plain text for the update dialog. */
public final class ReleaseNotes {
    private static final Pattern IMAGE = Pattern.compile("!\\[[^\\]]*\\]\\([^)]*\\)");
    private static final Pattern LINK = Pattern.compile("\\[([^\\]]*)\\]\\([^)]*\\)");
    private static final Pattern HTML = Pattern.compile("<[^>]+>");
    // [ \t] rather than \s: a pattern must not reach across a line break.
    private static final Pattern HEADING = Pattern.compile("(?m)^[ \\t]{0,3}#{1,6}[ \\t]*");
    private static final Pattern QUOTE = Pattern.compile("(?m)^[ \\t]*>[ \\t]?");
    private static final Pattern BULLET = Pattern.compile("(?m)^([ \\t]*)[-*+][ \\t]+");
    private static final Pattern EMPHASIS = Pattern.compile("(\\*\\*|__|~~|`)");
    private static final Pattern BLANK_RUNS = Pattern.compile("\\n{3,}");

    private ReleaseNotes() {
    }

    public static String plain(String markdown, int maxChars) {
        if (markdown == null) {
            return "";
        }
        String text = markdown.replace("\r\n", "\n").replace('\r', '\n');
        text = IMAGE.matcher(text).replaceAll("");
        text = LINK.matcher(text).replaceAll("$1");
        text = HTML.matcher(text).replaceAll("");
        text = HEADING.matcher(text).replaceAll("");
        text = QUOTE.matcher(text).replaceAll("");
        text = BULLET.matcher(text).replaceAll("$1• ");
        text = EMPHASIS.matcher(text).replaceAll("");
        text = BLANK_RUNS.matcher(text).replaceAll("\n\n").trim();
        if (text.length() > maxChars) {
            int cut = Math.max(0, maxChars - 1);
            // Do not split a surrogate pair.
            if (cut > 0 && Character.isHighSurrogate(text.charAt(cut - 1))) {
                cut--;
            }
            text = text.substring(0, cut).trim() + "…";
        }
        return text;
    }
}
