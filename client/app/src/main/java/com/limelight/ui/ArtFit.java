package com.limelight.ui;

import android.graphics.Bitmap;
import android.widget.ImageView;

/**
 * How a game's cover fills its frame: always cropped to fill it, centred, never letterboxed and never stretched. A host
 * sends whatever art the store has (2:3 boxart, 3:4 boxes, banners, squares); every frame shows the middle of it, edge to
 * edge, the way Nightjar's desktop client draws covers ({@code object-fit: cover}), so both apps match.
 */
public final class ArtFit {
    private ArtFit() { }

    /** A view about to be bound to a cover crops again, even if its last bind showed an icon centred inside it. */
    public static void reset(ImageView image) {
        image.setScaleType(ImageView.ScaleType.CENTER_CROP);
    }

    /** Shows a decoded cover cropped to fill its frame. */
    public static void show(ImageView image, Bitmap art) {
        image.setScaleType(ImageView.ScaleType.CENTER_CROP);
        image.setImageBitmap(art);
    }
}
