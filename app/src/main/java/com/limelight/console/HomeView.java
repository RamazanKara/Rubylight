package com.limelight.console;

import android.content.SharedPreferences;

/**
 * How Home lays out its games: a grid that scrolls down or shelves that scroll sideways, the library in one part or a
 * part per store, and the size of the cards. Nothing stored means the device's own look: the grid of one library on a
 * phone or tablet, the shelves per store on a TV.
 */
public final class HomeView {
    /**
     * Covers in a grid, a line per game with its store and when it was last played, or shelves of covers. The View
     * sheet offers them in this order; they are stored by name.
     */
    public enum Layout { GRID, LIST, ROWS }

    /**
     * A part per store, the games apart from the apps of no store (launchers, the desktop), a part per collection the
     * PCs name, or one library. Stored by name.
     */
    public enum Group { STORE, TYPE, COLLECTION, NONE }

    public enum Size {
        SMALL(0.75f), MEDIUM(1f), LARGE(1.3f);

        /** A card's width against the medium card's. */
        public final float scale;

        Size(float scale) {
            this.scale = scale;
        }
    }

    public static final String KEY_LAYOUT = "home_view_layout";
    public static final String KEY_GROUP = "home_view_group";
    public static final String KEY_SIZE = "home_view_size";

    public final Layout layout;
    public final Group group;
    public final Size size;

    public HomeView(Layout layout, Group group, Size size) {
        this.layout = layout;
        this.group = group;
        this.size = size;
    }

    /** The look Home had before it could be changed: the phone's grid, the TV's shelves per store. */
    public static HomeView defaults(boolean television) {
        return television ? new HomeView(Layout.ROWS, Group.STORE, Size.MEDIUM)
                : new HomeView(Layout.GRID, Group.NONE, Size.MEDIUM);
    }

    /** The stored view; a part never chosen, or a value this version does not know, keeps the device's default. */
    public static HomeView read(SharedPreferences prefs, boolean television) {
        HomeView base = defaults(television);
        return new HomeView(parse(Layout.class, prefs.getString(KEY_LAYOUT, null), base.layout),
                parse(Group.class, prefs.getString(KEY_GROUP, null), base.group),
                parse(Size.class, prefs.getString(KEY_SIZE, null), base.size));
    }

    public void save(SharedPreferences prefs) {
        prefs.edit().putString(KEY_LAYOUT, layout.name().toLowerCase(java.util.Locale.ROOT))
                .putString(KEY_GROUP, group.name().toLowerCase(java.util.Locale.ROOT))
                .putString(KEY_SIZE, size.name().toLowerCase(java.util.Locale.ROOT)).apply();
    }

    /** The groupings the View sheet offers: Collection only while the PCs name any. */
    public static Group[] offeredGroups(boolean collections) {
        return collections ? Group.values() : new Group[] {Group.STORE, Group.TYPE, Group.NONE};
    }

    public HomeView with(Layout next) {
        return new HomeView(next, group, size);
    }

    public HomeView with(Group next) {
        return new HomeView(layout, next, size);
    }

    public HomeView with(Size next) {
        return new HomeView(layout, group, next);
    }

    private static <E extends Enum<E>> E parse(Class<E> type, String stored, E fallback) {
        if (stored == null) return fallback;
        for (E value : type.getEnumConstants()) {
            if (value.name().equalsIgnoreCase(stored)) return value;
        }
        return fallback;
    }

    @Override
    public boolean equals(Object other) {
        if (!(other instanceof HomeView)) return false;
        HomeView view = (HomeView) other;
        return view.layout == layout && view.group == group && view.size == size;
    }

    @Override
    public int hashCode() {
        return (layout.ordinal() * 31 + group.ordinal()) * 31 + size.ordinal();
    }

    @Override
    public String toString() {
        return layout + "/" + group + "/" + size;
    }
}
