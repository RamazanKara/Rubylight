package com.limelight.ui;

import android.content.SharedPreferences;
import android.content.pm.ActivityInfo;
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

    /** The panel is taller than wide at rotation 0 (a phone; a tablet or handheld is not). */
    public static boolean naturalPortrait(Display display) {
        Point size = new Point();
        display.getRealSize(size);
        boolean turned = display.getRotation() == Surface.ROTATION_90 || display.getRotation() == Surface.ROTATION_270;
        return turned ? size.x > size.y : size.y > size.x;
    }

    /**
     * The rotation a stream started now shows in, which Settings edits the offset in: Settings is
     * usually held upright while a phone streams in landscape, so Left / right there must mean
     * left and right in the stream. Takes the orientation the stream window will ask for
     * ({@link StreamOrientation#requested}, so the rule is not repeated here): a landscape request
     * keeps the landscape shown or takes the system's, reverse landscape its opposite, a portrait
     * request the portrait shown or the system's, and a window that turns freely the rotation now.
     * Flip 180 degrees is per stream: the stored offset is natural, so it mirrors with the cutout.
     *
     * @param requested an {@code ActivityInfo.SCREEN_ORIENTATION_*} constant
     */
    public static int streamRotation(int rotation, boolean naturalPortrait, int requested) {
        int landscape = naturalPortrait ? Surface.ROTATION_90 : Surface.ROTATION_0;
        int portrait = naturalPortrait ? Surface.ROTATION_0 : Surface.ROTATION_270;
        boolean nowLandscape = rotation == landscape || rotation == landscape + 2;
        switch (requested) {
            case ActivityInfo.SCREEN_ORIENTATION_REVERSE_LANDSCAPE:
                return landscape + 2;
            case ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE:
            case ActivityInfo.SCREEN_ORIENTATION_USER_LANDSCAPE:
            case ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE:
                return nowLandscape ? rotation : landscape;
            case ActivityInfo.SCREEN_ORIENTATION_PORTRAIT:
            case ActivityInfo.SCREEN_ORIENTATION_USER_PORTRAIT:
            case ActivityInfo.SCREEN_ORIENTATION_SENSOR_PORTRAIT:
                return nowLandscape ? portrait : rotation;
            default:
                return rotation;
        }
    }

    /** The offset on the screen as it is turned now, in thousandths of each side: {x, y}. */
    public static int[] screenOffset(SharedPreferences prefs, Display display) {
        return screenOffset(prefs, display, display.getRotation());
    }

    /** The offset on the screen turned to this rotation: {x, y}. */
    public static int[] screenOffset(SharedPreferences prefs, Display display, int rotation) {
        int[] natural = parse(prefs.getString(PREF, null), screenKey(display));
        return new int[] {SafeAreaPolicy.screenOffsetX(natural[0], natural[1], rotation),
                SafeAreaPolicy.screenOffsetY(natural[0], natural[1], rotation)};
    }

    /** Saves an offset seen on the screen turned to this rotation. */
    public static void saveScreenOffset(SharedPreferences prefs, Display display, int rotation, int x, int y) {
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
