package com.limelight.utils;

import java.io.BufferedReader;
import java.io.CharArrayReader;
import java.io.IOException;
import java.io.Reader;
import java.io.StringReader;
import java.io.StringWriter;
import java.util.HashMap;
import java.util.Locale;
import java.util.Map;

import javax.xml.parsers.DocumentBuilder;
import javax.xml.parsers.DocumentBuilderFactory;
import javax.xml.parsers.ParserConfigurationException;
import javax.xml.transform.TransformerException;
import javax.xml.transform.TransformerFactory;
import javax.xml.transform.dom.DOMSource;
import javax.xml.transform.stream.StreamResult;

import org.w3c.dom.Document;
import org.w3c.dom.Element;
import org.w3c.dom.Node;
import org.xml.sax.InputSource;
import org.xml.sax.SAXException;

import com.limelight.BuildConfig;

/**
 * Game entry files for ES-DE, Daijisho, Pegasus and other Android frontends.
 *
 * The format matches Artemis' ".art" files, so entries exported by Apollo tools also launch here:
 * "#" comment lines, then one "[key] value" pair per line. Frontends pass the file to
 * {@link com.limelight.ShortcutTrampoline} as an ACTION_VIEW content URI.
 */
public final class FrontendEntry {
    public static final String EXTENSION = ".art";
    public static final String KEY_HOST_UUID = "host_uuid";
    public static final String KEY_HOST_NAME = "host_name";
    public static final String KEY_APP_UUID = "app_uuid";
    public static final String KEY_APP_NAME = "app_name";
    public static final String KEY_APP_ID = "app_id";
    /** Marks an entry the launcher export wrote, so a later export may update or remove it. */
    public static final String KEY_EXPORTED = "nightjar_export";

    /** Entries are a few hundred bytes; anything larger is not one of ours. */
    public static final int MAX_CHARS = 16 * 1024;

    // ES-DE keys ROM and media folders by these names; renaming them would orphan existing setups.
    public static final String ES_SYSTEM_NAME = "butterpollo";
    public static final String ES_EMULATOR_NAME = "BUTTERPOLLO";

    private FrontendEntry() {}

    public static String serialize(String hostUuid, String hostName, String appUuid, String appName, int appId) {
        StringBuilder sb = new StringBuilder("# " + BuildConfig.BRAND_NAME + " game entry\n");
        appendLine(sb, KEY_HOST_UUID, hostUuid);
        appendLine(sb, KEY_HOST_NAME, hostName);
        appendLine(sb, KEY_APP_UUID, appUuid);
        appendLine(sb, KEY_APP_NAME, appName);
        if (appId > 0) {
            appendLine(sb, KEY_APP_ID, Integer.toString(appId));
        }
        return sb.toString();
    }

    /** An entry as the launcher export writes it: {@link #serialize} plus the export's marker. */
    public static String serializeExport(String hostUuid, String hostName, String appUuid, String appName, int appId) {
        StringBuilder sb = new StringBuilder(serialize(hostUuid, hostName, appUuid, appName, appId));
        appendLine(sb, KEY_EXPORTED, "1");
        return sb.toString();
    }

    /**
     * Whether the launcher export wrote this entry, so it may remove it: it carries the marker,
     * or it is exactly what an export before the marker wrote for one game. Entries a user wrote
     * (host only, app name only, edited) are never the export's.
     */
    static boolean writtenByExport(String raw, Map<String, String> values) {
        if ("1".equals(values.get(KEY_EXPORTED))) {
            return true;
        }
        String appId = values.get(KEY_APP_ID);
        if (appId == null || !appId.matches("[1-9][0-9]{0,8}")) {
            return false;
        }
        return raw.equals(serialize(values.get(KEY_HOST_UUID), values.get(KEY_HOST_NAME),
                values.get(KEY_APP_UUID), values.get(KEY_APP_NAME), Integer.parseInt(appId)));
    }

    private static void appendLine(StringBuilder sb, String key, String value) {
        if (value == null) {
            return;
        }
        // Values are single-line by construction; strip line breaks a host name might carry
        String clean = value.replace('\r', ' ').replace('\n', ' ').trim();
        if (!clean.isEmpty()) {
            sb.append('[').append(key).append("] ").append(clean).append('\n');
        }
    }

    /**
     * Parses an entry. Unknown keys are kept, malformed lines are rejected so a random file
     * handed to us by "Open with" never starts a stream.
     */
    public static Map<String, String> parse(Reader source) throws IOException {
        char[] contents = new char[MAX_CHARS + 1];
        int length = 0;
        int count;
        while ((count = source.read(contents, length, contents.length - length)) != -1) {
            length += count;
            if (length > MAX_CHARS) {
                throw new IOException("Entry file is too large");
            }
        }
        BufferedReader reader = new BufferedReader(new CharArrayReader(contents, 0, length));
        Map<String, String> values = new HashMap<>();
        String line;
        while ((line = reader.readLine()) != null) {
            line = line.trim();
            // Desktop editors may prefix the first line with a UTF-8 byte order mark.
            if (line.startsWith("\uFEFF")) {
                line = line.substring(1).trim();
            }
            if (line.isEmpty() || line.startsWith("#")) {
                continue;
            }
            int close = line.indexOf(']');
            if (!line.startsWith("[") || close < 2) {
                throw new IOException("Not a game entry file");
            }
            String key = line.substring(1, close).trim().toLowerCase(Locale.ROOT);
            String value = line.substring(close + 1).trim();
            if (!value.isEmpty()) {
                values.put(key, value);
            }
        }
        if (!values.containsKey(KEY_HOST_UUID) && !values.containsKey(KEY_HOST_NAME)) {
            throw new IOException("Entry names no host");
        }
        return values;
    }

    /** A file name that works on FAT/exFAT SD cards and in frontends' file lists. */
    public static String fileBaseName(String appName) {
        String name = appName == null ? "" : appName;
        StringBuilder sb = new StringBuilder(name.length());
        for (int i = 0; i < name.length(); i++) {
            char c = name.charAt(i);
            if (c < 0x20 || "\\/:*?\"<>|".indexOf(c) >= 0) {
                sb.append(' ');
            } else {
                sb.append(c);
            }
        }
        String base = sb.toString().replaceAll("\\s+", " ").trim();
        // Leading dots hide files; trailing dots and spaces are dropped by FAT
        base = base.replaceAll("^[.\\s]+", "").replaceAll("[.\\s]+$", "");
        int end = 0;
        int bytes = 0;
        while (end < base.length()) {
            int codePoint = base.codePointAt(end);
            int chars = Character.charCount(codePoint);
            int encodedBytes = codePoint < 0x80 ? 1 : codePoint < 0x800 ? 2 : codePoint < 0x10000 ? 3 : 4;
            // Leave room for a collision suffix and extension on filesystems with a 255-byte limit.
            if (end + chars > 120 || bytes + encodedBytes > 200) {
                break;
            }
            end += chars;
            bytes += encodedBytes;
        }
        base = base.substring(0, end).replaceAll("[.\\s]+$", "");
        return base.isEmpty() ? "Game" : base;
    }

    public static String esSystemBlock(String hostLabel) {
        return "    <system>\n" +
                "        <name>" + ES_SYSTEM_NAME + "</name>\n" +
                "        <fullname>" + xmlEscape(hostLabel) + "</fullname>\n" +
                "        <path>%ROMPATH%/" + ES_SYSTEM_NAME + "</path>\n" +
                "        <extension>" + EXTENSION + " " + EXTENSION.toUpperCase(Locale.ROOT) + "</extension>\n" +
                "        <command label=\"" + BuildConfig.BRAND_NAME + "\">%EMULATOR_" + ES_EMULATOR_NAME + "% %ACTIVITY_CLEAR_TASK% %ACTIVITY_CLEAR_TOP% " +
                "%ACTION%=android.intent.action.VIEW %DATA%=%ROMPROVIDER%</command>\n" +
                "        <platform>pc</platform>\n" +
                "        <theme>windows</theme>\n" +
                "    </system>\n";
    }

    public static String esFindRuleBlock(String packageName) {
        return "    <emulator name=\"" + ES_EMULATOR_NAME + "\">\n" +
                "        <!-- " + BuildConfig.BRAND_NAME + " game streaming client -->\n" +
                "        <rule type=\"androidpackage\">\n" +
                "            <entry>" + xmlEscape(packageName) + "/com.limelight.ShortcutTrampoline</entry>\n" +
                "        </rule>\n" +
                "    </emulator>\n";
    }

    /** Adds or replaces our system in an existing ES-DE custom_systems/es_systems.xml. */
    public static String mergeEsSystems(String existing, String hostLabel) {
        return mergeBlock(existing, "systemList", "system", ES_SYSTEM_NAME, esSystemBlock(hostLabel));
    }

    /** Adds or replaces our emulator in an existing ES-DE custom_systems/es_find_rules.xml. */
    public static String mergeEsFindRules(String existing, String packageName) {
        return mergeBlock(existing, "ruleList", "emulator", ES_EMULATOR_NAME, esFindRuleBlock(packageName));
    }

    private static String mergeBlock(String existing, String root, String tag, String name, String block) {
        if (existing == null || existing.trim().isEmpty()) {
            return "<?xml version=\"1.0\"?>\n<" + root + ">\n" + block + "</" + root + ">\n";
        }
        // ES-DE files need no DTD, and must never resolve entities from files or the network.
        if (existing.contains("<!DOCTYPE")) {
            throw new IllegalArgumentException("ES-DE XML must not contain a DTD");
        }
        try {
            DocumentBuilder builder = DocumentBuilderFactory.newInstance().newDocumentBuilder();
            builder.setEntityResolver((publicId, systemId) -> {
                throw new SAXException("External entities are not supported");
            });
            Document document = builder.parse(new InputSource(new StringReader(existing)));
            Element parent = document.getDocumentElement();
            if (!root.equals(parent.getTagName())) {
                throw new IllegalArgumentException("Unexpected ES-DE XML root");
            }
            Node replacement = document.importNode(
                    builder.parse(new InputSource(new StringReader(block))).getDocumentElement(), true);
            boolean replaced = false;
            for (Node child = parent.getFirstChild(); child != null;) {
                Node next = child.getNextSibling();
                if (child instanceof Element && tag.equals(child.getNodeName())) {
                    String childName = ((Element) child).getAttribute("name");
                    if (tag.equals("system")) {
                        for (Node field = child.getFirstChild(); field != null; field = field.getNextSibling()) {
                            if (field instanceof Element && field.getNodeName().equals("name")) {
                                childName = field.getTextContent().trim();
                                break;
                            }
                        }
                    }
                    if (name.equals(childName)) {
                        if (replaced) {
                            parent.removeChild(child);
                        } else {
                            parent.replaceChild(replacement, child);
                            replaced = true;
                        }
                    }
                }
                child = next;
            }
            if (!replaced) {
                parent.appendChild(document.createTextNode("\n    "));
                parent.appendChild(replacement);
                parent.appendChild(document.createTextNode("\n"));
            }
            StringWriter result = new StringWriter();
            TransformerFactory.newInstance().newTransformer().transform(new DOMSource(document), new StreamResult(result));
            return result.toString().replace("\r\n", "\n");
        } catch (ParserConfigurationException | SAXException | IOException | TransformerException e) {
            throw new IllegalArgumentException("Unable to merge ES-DE XML", e);
        }
    }

    static String xmlEscape(String value) {
        if (value == null) {
            return "";
        }
        return value.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
                .replace("\"", "&quot;").replace("'", "&apos;");
    }

    static boolean matchesApp(Map<String, String> values, String hostUuid, String appUuid, int appId) {
        if (!hostUuid.equalsIgnoreCase(values.get(KEY_HOST_UUID))) {
            return false;
        }
        String entryUuid = values.get(KEY_APP_UUID);
        return entryUuid != null ? entryUuid.equalsIgnoreCase(appUuid) :
                appId > 0 && Integer.toString(appId).equals(values.get(KEY_APP_ID));
    }
}
