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
     * left and right in the stream. Landscape modes (and Automatic on a phone) take the system's
     * landscape, or the one already shown; Reverse landscape its opposite; Portrait the portrait
     * shown or the system's; Rotate, and Automatic where the stream turns freely, the rotation now.
     * Flip 180 degrees is per stream: the stored offset is natural, so it mirrors with the cutout.
     *
     * @param turnsFreely Automatic lets the window turn: a large, desktop-class or squarish screen
     */
    public static int streamRotation(int rotation, boolean naturalPortrait, int orientationMode, boolean turnsFreely) {
        int landscape = naturalPortrait ? Surface.ROTATION_90 : Surface.ROTATION_0;
        int portrait = naturalPortrait ? Surface.ROTATION_0 : Surface.ROTATION_270;
        boolean nowLandscape = rotation == landscape || rotation == landscape + 2;
        switch (orientationMode) {
            case StreamOrientation.ROTATE:
                return rotation;
            case StreamOrientation.PORTRAIT:
                return nowLandscape ? portrait : rotation;
            case StreamOrientation.REVERSE_LANDSCAPE:
                return landscape + 2;
            case StreamOrientation.LANDSCAPE:
            case StreamOrientation.LANDSCAPE_FLIP:
                return nowLandscape ? rotation : landscape;
            default:
                return turnsFreely || nowLandscape ? rotation : landscape;
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
