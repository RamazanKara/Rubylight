/**
 * The Control Center's model, the same one desktop Nightjar draws from: nightjar-policy's {@code control_center}
 * (protocol/policy), reached through {@link NightjarUiPolicyJni#controlCenterCall}. It sorts what the stream offers into
 * the modules of the rail, says which ways out stand under every page, where a step along the rail lands, where a
 * controller button's card is, where the Control Center sits on this screen and what it is drawn on.
 */
package com.limelight.ui;

import com.limelight.nvstream.jni.NightjarUiPolicyJni;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.Set;

public final class ControlCenterModel {
    private ControlCenterModel() { }

    /** The modules, by the policy's keys, in rail order. */
    public static final String QUICK = "quick", PICTURE = "picture", AUDIO = "audio", CLIPS = "clips", PC = "pc",
            CONTROLLER = "controller", GUESTS = "guests", POWER = "power";
    public static final List<String> MODULES = Collections.unmodifiableList(java.util.Arrays.asList(
            QUICK, PICTURE, AUDIO, CLIPS, PC, CONTROLLER, GUESTS, POWER));

    /** {@link #placement}'s answers. */
    public static final String SHEET = "sheet", DOCK = "dock";
    /** {@link #surface}'s answers; null while closed. */
    public static final String BLURRED = "blurred", BAKED = "baked", SOLID = "solid";

    /** One module's page: its cards in order, then the rows under them. */
    public static final class Page {
        public final String module;
        public final List<StreamMenuModel.Cell> cards;
        public final List<String> rows;

        Page(String module, List<StreamMenuModel.Cell> cards, List<String> rows) {
            this.module = module;
            this.cards = Collections.unmodifiableList(cards);
            this.rows = Collections.unmodifiableList(rows);
        }

        /** Whether a tile with this id is one of the page's cards or rows. */
        public boolean holds(String id) {
            for (StreamMenuModel.Cell card : cards) if (id.equals(card.id)) return true;
            return rows.contains(id);
        }
    }

    /** A controller button's card: its module and its place among that page's cards. */
    public static final class Place {
        public final String module;
        public final int card;

        Place(String module, int card) {
            this.module = module;
            this.card = card;
        }
    }

    /** The Control Center for what a stream offers. */
    public static final class Dock {
        /** The modules that have something, in rail order; Quick is always there. */
        public final List<Page> pages;
        /** Close, then the ways out under every page. */
        public final List<String> footer;
        /** The tile id each controller button runs now. */
        public final Map<String, String> buttons;
        // What the dock was asked with: a step and a button's place are answers about the same offers.
        private final JSONObject request;

        Dock(List<Page> pages, List<String> footer, Map<String, String> buttons, JSONObject request) {
            this.pages = Collections.unmodifiableList(pages);
            this.footer = Collections.unmodifiableList(footer);
            this.buttons = Collections.unmodifiableMap(buttons);
            this.request = request;
        }

        /** The page of a module, or null for one the rail does not show. */
        public Page page(String module) {
            for (Page page : pages) if (page.module.equals(module)) return page;
            return null;
        }

        /** The rail: the modules shown, in order. */
        public List<String> modules() {
            List<String> modules = new ArrayList<>();
            for (Page page : pages) modules.add(page.module);
            return modules;
        }

        /** The module that holds a tile, or null for a way out under the pages. */
        public String moduleOf(String id) {
            for (Page page : pages) if (page.holds(id)) return page.module;
            return null;
        }

        /**
         * The module a trigger or a rail arrow lands on from {@code from}: the next one shown, stopping at both ends, and
         * the first for a module the rail does not show.
         */
        public String step(String from, boolean forward) throws StreamMenuModel.Refused {
            try {
                JSONObject ask = new JSONObject(request.toString()).put("op", "step").put("from", from).put("forward", forward);
                return (String) ControlCenterModel.ask(ask);
            } catch (JSONException | ClassCastException e) {
                throw new StreamMenuModel.Refused("invalid", "unreadable step: " + e.getMessage());
            }
        }

        /** Where the card of a controller button ({@code "y"}, {@code "x"}, {@code "lb"}, {@code "rb"}) is, or null. */
        public Place hotkey(String key) throws StreamMenuModel.Refused {
            try {
                Object answer = ControlCenterModel.ask(new JSONObject(request.toString()).put("op", "hotkey").put("key", key));
                if (answer == null || answer == JSONObject.NULL) return null;
                JSONArray place = (JSONArray) answer;
                return new Place(place.getString(0), place.getInt(1));
            } catch (JSONException | ClassCastException e) {
                throw new StreamMenuModel.Refused("invalid", "unreadable hotkey: " + e.getMessage());
            }
        }
    }

    private static Object ask(JSONObject request) throws StreamMenuModel.Refused {
        String answer;
        try {
            answer = NightjarUiPolicyJni.controlCenterCall(request.toString());
        } catch (UnsatisfiedLinkError e) {
            // A library from another build that lacks the call: the Control Center says so instead of crashing the stream.
            throw new StreamMenuModel.Refused("unavailable", String.valueOf(e.getMessage()));
        } catch (LinkageError | RuntimeException e) {
            throw new StreamMenuModel.Refused("unavailable", String.valueOf(e));
        }
        if (answer == null) throw new StreamMenuModel.Refused("unavailable", "no answer");
        try {
            JSONObject reply = new JSONObject(answer);
            // A null answer is an answer: no card for a button, nothing drawn while closed.
            if (reply.has("ok")) return reply.isNull("ok") ? null : reply.get("ok");
            throw new StreamMenuModel.Refused(reply.optString("error", "invalid"),
                    reply.isNull("detail") ? "" : reply.optString("detail", ""));
        } catch (JSONException e) {
            throw new StreamMenuModel.Refused("invalid", "unreadable answer: " + e.getMessage());
        }
    }

    /**
     * The Control Center for these offers. {@code shortcuts} is how many the user saved, or null where they cannot be
     * sent; {@code buttons} the user's choices for the controller's buttons; {@code gates} the later PC features this PC
     * advertises ({@code stream_dvr}, {@code frame_warp}, ...), each off unless named.
     */
    public static Dock dock(List<StreamMenuModel.Offer> offers, Integer shortcuts, Map<String, String> buttons,
                            Set<String> gates) throws StreamMenuModel.Refused {
        try {
            JSONArray list = new JSONArray();
            for (StreamMenuModel.Offer offer : offers) {
                JSONObject item = new JSONObject().put("id", offer.id);
                if (offer.on != null) item.put("on", offer.on.booleanValue());
                list.put(item);
            }
            JSONObject request = new JSONObject().put("op", "dock").put("offers", list);
            if (shortcuts != null) request.put("shortcuts", shortcuts.intValue());
            if (buttons != null && !buttons.isEmpty()) request.put("buttons", new JSONObject(buttons));
            if (gates != null && !gates.isEmpty()) {
                JSONObject open = new JSONObject();
                for (String gate : gates) open.put(gate, true);
                request.put("gates", open);
            }
            JSONObject answer = (JSONObject) ask(request);
            List<Page> pages = new ArrayList<>();
            JSONArray modules = answer.getJSONArray("pages");
            for (int i = 0; i < modules.length(); i++) {
                JSONObject page = modules.getJSONObject(i);
                List<StreamMenuModel.Cell> cards = new ArrayList<>();
                JSONArray cells = page.getJSONArray("cards");
                for (int at = 0; at < cells.length(); at++) cards.add(cell(cells.get(at)));
                List<String> rows = new ArrayList<>();
                JSONArray ids = page.getJSONArray("rows");
                for (int at = 0; at < ids.length(); at++) rows.add(ids.getString(at));
                pages.add(new Page(page.getString("module"), cards, rows));
            }
            List<String> footer = new ArrayList<>();
            JSONArray ways = answer.getJSONArray("footer");
            for (int i = 0; i < ways.length(); i++) footer.add(ways.getString(i));
            Map<String, String> running = new HashMap<>();
            JSONObject chosen = answer.getJSONObject("buttons");
            for (Iterator<String> keys = chosen.keys(); keys.hasNext(); ) {
                String key = keys.next();
                running.put(key, chosen.getString(key));
            }
            return new Dock(pages, footer, running, request);
        } catch (JSONException | ClassCastException | NullPointerException e) {
            throw new StreamMenuModel.Refused("invalid", "unreadable dock: " + e.getMessage());
        }
    }

    private static StreamMenuModel.Cell cell(Object cell) throws JSONException {
        if ("add".equals(cell)) return new StreamMenuModel.Cell(StreamMenuModel.Cell.Kind.ADD, null, null, null, -1);
        JSONObject object = (JSONObject) cell;
        if (object.has("shortcut")) {
            return new StreamMenuModel.Cell(StreamMenuModel.Cell.Kind.SHORTCUT, null, null, null, object.getInt("shortcut"));
        }
        JSONObject tile = object.getJSONObject("tile");
        return new StreamMenuModel.Cell(StreamMenuModel.Cell.Kind.TILE, tile.getString("id"),
                tile.has("on") && !tile.isNull("on") ? tile.getBoolean("on") : null,
                tile.isNull("hotkey") ? null : tile.optString("hotkey", null), -1);
    }

    /**
     * Where the Control Center sits in a window of this form ({@link ViewportScaler#PHONE}, ...) and size: {@link #SHEET}
     * from the bottom on an upright phone, {@link #DOCK} at the end edge on everything wide.
     */
    public static String placement(String form, float widthDp, float heightDp) {
        try {
            Object answer = ask(new JSONObject().put("op", "placement").put("form", form)
                    .put("width_dp", widthDp).put("height_dp", heightDp));
            if (SHEET.equals(answer) || DOCK.equals(answer)) return (String) answer;
        } catch (JSONException | StreamMenuModel.Refused e) {
            com.limelight.LimeLog.warning("Control Center: the placement failed: " + e.getMessage());
        }
        return placementMirror(form, widthDp, heightDp);
    }

    private static final float DOCK_MIN_WINDOW_DP = 560f;

    /** control_center::placement in Java, for a library that cannot answer; pinned to the policy by a test. */
    static String placementMirror(String form, float widthDp, float heightDp) {
        return ViewportScaler.PHONE.equals(form) && widthDp < DOCK_MIN_WINDOW_DP && heightDp > widthDp ? SHEET : DOCK;
    }

    /**
     * What the open Control Center is drawn on right now: {@link #BLURRED} only where the interface's tier blurs and the
     * system allows it at this moment, {@link #SOLID} under high contrast, else {@link #BAKED}. Null while closed. A
     * library that cannot answer never blurs.
     */
    public static String surface(boolean open, String tier, boolean blurAvailable, boolean powerSave, boolean thermalSevere,
                                 boolean highContrast) {
        try {
            Object answer = ask(new JSONObject().put("op", "surface").put("open", open).put("tier", tier)
                    .put("blur_available", blurAvailable).put("power_save", powerSave)
                    .put("thermal_severe", thermalSevere).put("high_contrast", highContrast));
            if (answer == null) return null;
            if (BLURRED.equals(answer) || BAKED.equals(answer) || SOLID.equals(answer)) return (String) answer;
        } catch (JSONException | StreamMenuModel.Refused e) {
            com.limelight.LimeLog.warning("Control Center: the surface failed: " + e.getMessage());
        }
        return open ? (highContrast ? SOLID : BAKED) : null;
    }
}
