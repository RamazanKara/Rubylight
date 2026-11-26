package com.limelight.ui;

import android.content.SharedPreferences;
import android.graphics.Point;
import android.view.Display;
import android.view.Surface;

/**
 * Picture position: how far the player moved the picture by hand to make up for a camera cutout
 * or curved edges, saved for each screen of this device (a foldable's inner and cover screens
 * have different cutouts). One preference holds every screen's offset as
 * {@code "<width>x<height>:<x>,<y>;..."}, with the screen's size and the offset in its natural
 * orientation (see {@link SafeAreaPolicy}); a screen at the centre has no entry.
 */
public final class PicturePosition {
    public static final String PREF = "picture_position";

    private PicturePosition() {}

    /** The screen's identity: its real size in its natural orientation. */
    public static String screenKey(Display display) {
        Point size = new Point();
        display.getRealSize(size);
        boolean turned = display.getRotation() == Surface.ROTATION_90 || display.getRotation() == Surface.ROTATION_270;
        return turned ? size.y + "x" + size.x : size.x + "x" + size.y;
    }

    /** The offset on the screen as it is turned now, in thousandths of each side: {x, y}. */
    public static int[] screenOffset(SharedPreferences prefs, Display display) {
        int[] natural = parse(prefs.getString(PREF, null), screenKey(display));
        int rotation = display.getRotation();
        return new int[] {SafeAreaPolicy.screenOffsetX(natural[0], natural[1], rotation),
                SafeAreaPolicy.screenOffsetY(natural[0], natural[1], rotation)};
    }

    public static void saveScreenOffset(SharedPreferences prefs, Display display, int x, int y) {
        int rotation = display.getRotation();
        String stored = put(prefs.getString(PREF, null), screenKey(display),
                SafeAreaPolicy.naturalOffsetX(x, y, rotation), SafeAreaPolicy.naturalOffsetY(x, y, rotation));
        SharedPreferences.Editor editor = prefs.edit();
        if (stored.isEmpty()) editor.remove(PREF);
        else editor.putString(PREF, stored);
        editor.apply();
    }

    /** One screen's natural offset, {0, 0} when it has none or the value is damaged. */
    static int[] parse(String stored, String screen) {
        if (stored != null) {
            for (String entry : stored.split(";")) {
                int colon = entry.indexOf(':');
                if (colon < 0 || !entry.substring(0, colon).equals(screen)) continue;
                String[] xy = entry.substring(colon + 1).split(",");
                if (xy.length != 2) break;
                try {
                    return new int[] {SafeAreaPolicy.clampOffset(Integer.parseInt(xy[0].trim())),
                            SafeAreaPolicy.clampOffset(Integer.parseInt(xy[1].trim()))};
                } catch (NumberFormatException e) {
                    break;
                }
            }
        }
        return new int[] {0, 0};
    }

    /** The stored value with this screen's offset replaced; a centred screen loses its entry. */
    static String put(String stored, String screen, int x, int y) {
        StringBuilder out = new StringBuilder();
        if (stored != null) {
            for (String entry : stored.split(";")) {
                int colon = entry.indexOf(':');
                if (colon <= 0 || entry.substring(0, colon).equals(screen)) continue;
                if (out.length() > 0) out.append(';');
                out.append(entry);
            }
        }
        x = SafeAreaPolicy.clampOffset(x);
        y = SafeAreaPolicy.clampOffset(y);
        if (x != 0 || y != 0) {
            if (out.length() > 0) out.append(';');
            out.append(screen).append(':').append(x).append(',').append(y);
        }
        return out.toString();
    }
}
