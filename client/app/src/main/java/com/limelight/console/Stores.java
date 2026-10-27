package com.limelight.console;

import androidx.annotation.DrawableRes;

import com.limelight.R;

import java.util.HashMap;
import java.util.Map;

/**
 * The stores a host reads games from, by the key the host sends. Names are the stores' own product names and stay
 * untranslated; the host's name wins when it sends one. The glyphs are the stores' monochrome marks from Simple Icons
 * (CC0, see assets/NOTICE-SIMPLE-ICONS.txt), shown only beside the store's name, never instead of it.
 */
final class Stores {
    private static final Map<String, String> NAMES = new HashMap<>();
    private static final Map<String, Integer> GLYPHS = new HashMap<>();

    static {
        add("steam", "Steam", R.drawable.ic_store_steam);
        add("epic", "Epic Games", R.drawable.ic_store_epic);
        add("gog", "GOG", R.drawable.ic_store_gog);
        add("ea", "EA app", R.drawable.ic_store_ea);
        add("ubisoft", "Ubisoft Connect", R.drawable.ic_store_ubisoft);
        add("battlenet", "Battle.net", R.drawable.ic_store_battlenet);
        add("microsoft", "Xbox", R.drawable.ic_store_xbox);
    }

    private Stores() {
    }

    private static void add(String key, String name, int glyph) {
        NAMES.put(key, name);
        GLYPHS.put(key, glyph);
    }

    /** The store's name: the host's, else the known one, else the key itself; null without a store. */
    static String name(String key, String hostName) {
        if (hostName != null && !hostName.isEmpty()) return hostName;
        if (key == null) return null;
        String known = NAMES.get(key);
        return known != null ? known : key;
    }

    /** The store's glyph, or 0 for a store without one (its chip is its name). */
    @DrawableRes
    static int glyph(String key) {
        Integer glyph = key == null ? null : GLYPHS.get(key);
        return glyph == null ? 0 : glyph;
    }
}
