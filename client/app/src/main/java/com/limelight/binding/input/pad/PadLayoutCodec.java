package com.limelight.binding.input.pad;

import java.util.Locale;

/**
 * Forward- and backward-compatible text serialization and parsing for {@link PadLayout} (v1 format,
 * plus reading the short-lived v2 prefix).
 */
final class PadLayoutCodec {
    /**
     * Layouts are written as v1, which an older build still reads: it skips the controls, glyphs and pointer mode it
     * does not know instead of dropping the whole layout, so a downgrade keeps every control it has. The short-lived
     * v2 prefix this version used is still read.
     */
    private static final String VERSION = "v1";
    private static final String VERSION_V2 = "v2";
    /**
     * Written with a pointer touchpad that shows at its default place, which moved below the microphone and menu
     * buttons: a layout without it was saved when that place was the middle of the surface, and keeps it there.
     */
    private static final String POINTER_PAD_BELOW = "pad=below";
    /**
     * Written when the face cluster's buttons stand in the Nintendo arrangement ({@link PadLayout#facesSwapped}).
     * A token with no comma: older builds skip it as an entry with too few fields and show the standard arrangement.
     */
    private static final String FACES_SWAPPED = "faces=swapped";

    private PadLayoutCodec() {
    }

    /**
     * "v1|glyph=playstation|mode=camera|pad=below|ID,cx,cy,scale,opacity,hidden[,widthScale,heightScale]|..." for
     * edits; "v1" alone for none. A control whose width and height scales differ adds them after the six fields every version
     * reads, and writes the larger as its one scale, which an older build shows (up to its own limit of 2).
     */
    static String serialize(PadLayout layout) {
        StringBuilder out = new StringBuilder(VERSION);
        if (layout.glyph() != PadLayout.Glyph.XBOX) {
            out.append("|glyph=").append(layout.glyph().name().toLowerCase(Locale.ROOT));
        }
        if (layout.pointerMode() != PadLayout.PointerMode.MOUSE) {
            out.append("|mode=").append(layout.pointerMode().name().toLowerCase(Locale.ROOT));
        }
        if (layout.facesSwapped()) out.append('|').append(FACES_SWAPPED);
        PadLayout.Spec pad = layout.spec(PadLayout.Control.POINTER_PAD);
        if (!pad.hidden && !pad.placed()) out.append('|').append(POINTER_PAD_BELOW);
        for (PadLayout.Control control : PadLayout.Control.values()) {
            PadLayout.Spec spec = layout.spec(control);
            if (spec.isDefault()) continue;
            out.append('|').append(control.name()).append(',')
                    .append(spec.placed() ? String.format(Locale.ROOT, "%.4f", spec.cx) : "-").append(',')
                    .append(spec.placed() ? String.format(Locale.ROOT, "%.4f", spec.cy) : "-").append(',')
                    .append(String.format(Locale.ROOT, "%.3f", spec.scale())).append(',')
                    .append(spec.opacity).append(',')
                    .append(spec.hidden ? '1' : '0');
            if (spec.widthScale != spec.heightScale) {
                out.append(',').append(String.format(Locale.ROOT, "%.3f", spec.widthScale))
                        .append(',').append(String.format(Locale.ROOT, "%.3f", spec.heightScale));
            }
        }
        return out.toString();
    }

    /**
     * Reads {@link #serialize} output; "v2", the prefix a short-lived build wrote, loads too. The glyphs, the
     * pointer touchpad and its mode are optional: a layout saved before them loads as an Xbox layout with a mouse
     * touchpad. Unknown controls, unknown settings and broken entries are skipped and the rest kept, so a layout from
     * a newer version still loads what this one knows; an entry's fields past the eighth are ignored, as an older
     * build ignores the seventh and eighth. Null or another version gives the default layout.
     */
    static PadLayout parse(String text) {
        PadLayout layout = new PadLayout();
        if (text == null) return layout;
        String[] parts = text.split("\\|");
        if (parts.length == 0 || (!VERSION.equals(parts[0]) && !VERSION_V2.equals(parts[0]))) return layout;
        boolean padBelow = false;
        for (int i = 1; i < parts.length; i++) {
            String part = parts[i];
            if (part.equals(POINTER_PAD_BELOW)) {
                padBelow = true;
                continue;
            }
            if (part.startsWith("glyph=")) {
                try {
                    layout.setGlyph(PadLayout.Glyph.valueOf(part.substring("glyph=".length()).toUpperCase(Locale.ROOT)));
                } catch (IllegalArgumentException ignored) {
                    // A glyph from a newer build: Xbox, what this one shows.
                }
                continue;
            }
            if (part.equals(FACES_SWAPPED)) {
                layout.setFacesSwapped(true);
                continue;
            }
            if (part.startsWith("mode=")) {
                layout.setPointerMode("camera".equalsIgnoreCase(part.substring("mode=".length())) ?
                        PadLayout.PointerMode.CAMERA : PadLayout.PointerMode.MOUSE);
                continue;
            }
            String[] fields = part.split(",", -1);
            if (fields.length < 6) continue;
            PadLayout.Control control = PadLayout.Control.named(fields[0]);
            if (control == null) continue;
            try {
                PadLayout.Spec spec = new PadLayout.Spec();
                if (!"-".equals(fields[1]) && !"-".equals(fields[2])) {
                    spec.cx = PadLayout.clamp(Float.parseFloat(fields[1]), 0, 1);
                    spec.cy = PadLayout.clamp(Float.parseFloat(fields[2]), 0, 1);
                    if (Float.isNaN(spec.cx) || Float.isNaN(spec.cy)) {
                        spec.cx = Float.NaN;
                        spec.cy = Float.NaN;
                    }
                }
                spec.widthScale = spec.heightScale = PadLayout.clampScale(Float.parseFloat(fields[3]));
                spec.opacity = Math.max(PadLayout.MIN_OPACITY, Math.min(PadLayout.MAX_OPACITY, Integer.parseInt(fields[4])));
                spec.hidden = "1".equals(fields[5]);
                // A width and a height of their own, after the six fields; without both, the one scale serves both.
                if (fields.length >= 8) {
                    float w = parseScale(fields[6]), h = parseScale(fields[7]);
                    if (!Float.isNaN(w) && !Float.isNaN(h)) {
                        spec.widthScale = w;
                        spec.heightScale = h;
                    }
                }
                layout.spec(control).copyFrom(spec);
            } catch (NumberFormatException ignored) {
                // Skip this entry and keep the rest.
            }
        }
        PadLayout.Spec pad = layout.spec(PadLayout.Control.POINTER_PAD);
        if (!padBelow && !pad.hidden && !pad.placed()) {
            // Added before its default place moved and never dragged: it stays in the middle, where it was.
            pad.cx = .5f;
            pad.cy = .5f;
        }
        return layout;
    }

    /** A saved width or height scale, clamped; NaN when the field is not a number. */
    private static float parseScale(String field) {
        try {
            float scale = Float.parseFloat(field);
            return Float.isNaN(scale) ? Float.NaN : PadLayout.clampScale(scale);
        } catch (NumberFormatException e) {
            return Float.NaN;
        }
    }
}
