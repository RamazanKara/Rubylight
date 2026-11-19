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
     * PCs name, the installed games apart from the rest, or one library. Stored by name.
     */
    public enum Group { STORE, TYPE, COLLECTION, INSTALLATION, NONE }

    /** What the library's parts and All games sort by. Stored by name. */
    public enum Sort { TITLE, LAST_PLAYED, PLAYTIME }

    /** Which way: ascending is A to Z, oldest first and least played first. Stored by name. */
    public enum Order { ASCENDING, DESCENDING }

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
    public static final String KEY_SORT = "home_view_sort";
    public static final String KEY_ORDER = "home_view_order";

    public final Layout layout;
    public final Group group;
    public final Size size;
    public final Sort sort;
    public final Order order;

    public HomeView(Layout layout, Group group, Size size) {
        this(layout, group, size, Sort.TITLE, Order.ASCENDING);
    }

    public HomeView(Layout layout, Group group, Size size, Sort sort, Order order) {
        this.layout = layout;
        this.group = group;
        this.size = size;
        this.sort = sort;
        this.order = order;
    }

    /** The look Home had before it could be changed: the phone's grid, the TV's shelves per store. */
    public static HomeView defaults(boolean television) {
        return television ? new HomeView(Layout.ROWS, Group.STORE, Size.MEDIUM)
                : new HomeView(Layout.ROWS, Group.STORE, Size.MEDIUM);
    }

    /** The stored view; a part never chosen, or a value this version does not know, keeps the device's default. */
    public static HomeView read(SharedPreferences prefs, boolean television) {
        HomeView base = defaults(television);
        return new HomeView(parse(Layout.class, prefs.getString(KEY_LAYOUT, null), base.layout),
                parse(Group.class, prefs.getString(KEY_GROUP, null), base.group),
                parse(Size.class, prefs.getString(KEY_SIZE, null), base.size),
                parse(Sort.class, prefs.getString(KEY_SORT, null), base.sort),
                parse(Order.class, prefs.getString(KEY_ORDER, null), base.order));
    }

    public void save(SharedPreferences prefs) {
        prefs.edit().putString(KEY_LAYOUT, layout.name().toLowerCase(java.util.Locale.ROOT))
                .putString(KEY_GROUP, group.name().toLowerCase(java.util.Locale.ROOT))
                .putString(KEY_SIZE, size.name().toLowerCase(java.util.Locale.ROOT))
                .putString(KEY_SORT, sort.name().toLowerCase(java.util.Locale.ROOT))
                .putString(KEY_ORDER, order.name().toLowerCase(java.util.Locale.ROOT)).apply();
    }

    /** The groupings the View sheet offers: Collection only while the PCs name any. */
    public static Group[] offeredGroups(boolean collections) {
        return offeredGroups(collections, false);
    }

    /**
     * The groupings the View sheet offers: Collection only while the PCs name any, Installation only while the library
     * has games installed and games not.
     */
    public static Group[] offeredGroups(boolean collections, boolean installationSplit) {
        java.util.List<Group> offered = new java.util.ArrayList<>();
        for (Group group : Group.values()) {
            if (group == Group.COLLECTION && !collections) continue;
            if (group == Group.INSTALLATION && !installationSplit) continue;
            offered.add(group);
        }
        return offered.toArray(new Group[0]);
    }

    public HomeView with(Layout next) {
        return new HomeView(next, group, size, sort, order);
    }

    public HomeView with(Group next) {
        return new HomeView(layout, next, size, sort, order);
    }

    public HomeView with(Size next) {
        return new HomeView(layout, group, next, sort, order);
    }

    public HomeView with(Sort next) {
        return new HomeView(layout, group, size, next, order);
    }

    public HomeView with(Order next) {
        return new HomeView(layout, group, size, sort, next);
    }

    /** The order the merge already lists in: nothing to sort. */
    public boolean byTitle() {
        return sort == Sort.TITLE && order == Order.ASCENDING;
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
        return view.layout == layout && view.group == group && view.size == size && view.sort == sort
                && view.order == order;
    }

    @Override
    public int hashCode() {
        return (((layout.ordinal() * 31 + group.ordinal()) * 31 + size.ordinal()) * 31 + sort.ordinal()) * 31
                + order.ordinal();
    }

    @Override
    public String toString() {
        return layout + "/" + group + "/" + size + "/" + sort + "/" + order;
    }
}
