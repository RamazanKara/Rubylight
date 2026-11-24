package com.limelight.ui;

import android.view.KeyEvent;
import android.view.View;

import com.google.android.material.button.MaterialButton;
import com.limelight.binding.input.KeyboardShortcuts;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.TestHosts;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.assertNotNull;

/**
 * Shared fixtures, view-tree queries, and in-memory shortcut store for the {@link StreamMenu}
 * Robolectric test suites.
 */
final class StreamMenuTestSupport {
    private StreamMenuTestSupport() { }

    /** The Rubylight fixture on a 3.0 host that also speaks the native API. */
    static ComputerDetails nativeApiHost() {
        ComputerDetails host = TestHosts.details("rubylight-serverinfo.xml");
        host.caps = TestHosts.caps("<root status_code=\"200\"><RustHostVersion>3.0.0</RustHostVersion>"
                + "<PairStatus>1</PairStatus><NativeApiVersion>1</NativeApiVersion></root>");
        return host;
    }

    static StreamMenu.Options options(String fixture, NvApp.Role role) {
        return options(TestHosts.details(fixture), role);
    }

    static StreamMenu.Options options(ComputerDetails host, NvApp.Role role) {
        StreamMenu.Options options = new StreamMenu.Options(role, host);
        // As NvConnection decides: the host's input permission, never for Remote Monitor.
        options.canSendInput = role != NvApp.Role.REMOTE_MONITOR && host.hasPermission(ComputerDetails.PERMISSION_INPUT);
        options.webUiAddress = true;
        // Only Rubylight changes resolution and frame rate in place, which automatic quality needs.
        options.automaticQuality = host.caps.family() == com.limelight.nvstream.http.HostCapabilities.Family.RUBYLIGHT;
        return options;
    }

    static Map<StreamMenu.Section, List<StreamMenu.Item>> sections(List<StreamMenu.Entry> entries) {
        Map<StreamMenu.Section, List<StreamMenu.Item>> sections = new LinkedHashMap<>();
        for (StreamMenu.Entry entry : entries) {
            sections.computeIfAbsent(entry.section, ignored -> new ArrayList<>()).add(entry.item);
        }
        return sections;
    }

    /** The last of the session actions, which now sit under the quick toggles rather than at the end. */
    static StreamMenu.Entry lastSession(List<StreamMenu.Entry> entries) {
        StreamMenu.Entry last = null;
        for (StreamMenu.Entry entry : entries) {
            if (entry.section == StreamMenu.Section.SESSION) last = entry;
        }
        assertNotNull("the page has session actions", last);
        return last;
    }

    static StreamMenu.Entry entry(List<StreamMenu.Entry> entries, StreamMenu.Item item) {
        for (StreamMenu.Entry entry : entries) {
            if (entry.item == item) return entry;
        }
        throw new AssertionError(item + " is missing");
    }

    static List<StreamMenu.Item> items(List<StreamMenu.Entry> entries) {
        List<StreamMenu.Item> items = new ArrayList<>();
        for (StreamMenu.Entry entry : entries) items.add(entry.item);
        return items;
    }

    static boolean inScroll(View view) {
        for (android.view.ViewParent parent = view.getParent(); parent != null; parent = parent.getParent()) {
            if (parent instanceof androidx.core.widget.NestedScrollView) return true;
        }
        return false;
    }

    static void press(StreamMenu menu, int key) {
        menu.sheet().dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, key));
        menu.sheet().dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, key));
    }

    static String text(View tile) {
        return ((MaterialButton) tile).getText().toString();
    }

    // A button whose text starts with {@code start}; {@code outsideGrid} skips the tiles.
    static MaterialButton buttonStartingWith(View view, String start, boolean outsideGrid) {
        if (outsideGrid && view instanceof TileGrid) return null;
        if (view instanceof MaterialButton && text(view).startsWith(start)) return (MaterialButton) view;
        if (view instanceof android.view.ViewGroup) {
            for (int i = 0; i < ((android.view.ViewGroup) view).getChildCount(); i++) {
                MaterialButton found = buttonStartingWith(((android.view.ViewGroup) view).getChildAt(i), start, outsideGrid);
                if (found != null) return found;
            }
        }
        return null;
    }

    static boolean textShown(View view, String text) {
        if (view instanceof android.widget.TextView && view.getVisibility() == View.VISIBLE
                && ((android.widget.TextView) view).getText().toString().equals(text)) return true;
        if (view instanceof android.view.ViewGroup) {
            for (int i = 0; i < ((android.view.ViewGroup) view).getChildCount(); i++) {
                if (textShown(((android.view.ViewGroup) view).getChildAt(i), text)) return true;
            }
        }
        return false;
    }

    static TileGrid findGrid(View view) {
        if (view instanceof TileGrid) return (TileGrid) view;
        if (view instanceof android.view.ViewGroup) {
            for (int i = 0; i < ((android.view.ViewGroup) view).getChildCount(); i++) {
                TileGrid found = findGrid(((android.view.ViewGroup) view).getChildAt(i));
                if (found != null) return found;
            }
        }
        return null;
    }

    static MaterialButton capNamed(TileGrid caps, String label) {
        for (int i = 0; i < caps.getChildCount(); i++) {
            if (text(caps.getChildAt(i)).equals(label)) return (MaterialButton) caps.getChildAt(i);
        }
        throw new AssertionError("no key " + label);
    }

    /** The user's shortcuts in memory, and what the menu sent. */
    static final class SavedShortcuts implements StreamMenu.Shortcuts {
        final List<KeyboardShortcuts.Shortcut> list = new ArrayList<>();
        final List<int[]> sent = new ArrayList<>();

        SavedShortcuts add(String name, String chord) {
            list.add(new KeyboardShortcuts.Shortcut("s" + list.size(), name, KeyboardShortcuts.parseChord(chord)));
            return this;
        }

        @Override public List<KeyboardShortcuts.Shortcut> saved() {
            return new ArrayList<>(list);
        }

        @Override public void save(List<KeyboardShortcuts.Shortcut> shortcuts) {
            list.clear();
            list.addAll(shortcuts);
        }

        @Override public void send(int[] keys) {
            sent.add(keys);
        }
    }
}
