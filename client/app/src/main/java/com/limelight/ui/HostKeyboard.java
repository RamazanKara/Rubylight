package com.limelight.ui;

/**
 * Who the soft keyboard that follows the host's text fields (0x553B notices) belongs to, and where
 * the stream sits while it is open. A raise opens it only right after the user's own tap, click or
 * key on the stream, so a game field that takes the focus by itself leaves it closed; a keyboard
 * the user closed stays closed for that field; and while it is open the stream moves up so the
 * point last pressed stays above it, without resizing the video surface.
 *
 * A host from TextInputVersion 2 names the focused element with an id that stays the same while the
 * element lives, so a keyboard closed on one field stays closed for that field, also when the focus
 * comes back to it, and opens for any other. An older host sends no id; it counts on each time its
 * focus moves to another element and sends an update for it, and Nightjar never asks for the text
 * around the cursor, so there every raise and update is another field.
 *
 * Ideas from Moonlight V+ (qiin2333/moonlight-vplus, GPL-3.0): auto-showing only for focus that
 * follows the user's input, never reopening against a dismissal of the same activation, and its
 * ImeAvoidanceSession / RemoteTextContextPolicy viewport offset, locked to one target for each
 * keyboard cycle and taken back when the keyboard closes.
 */
public final class HostKeyboard {
    /** A raise opens the keyboard only this soon after the user's own input on the stream. */
    public static final long INPUT_WINDOW_MS = 1000;
    /** A press older than this when the keyboard opens is not what it types into (V+'s MAX_PENDING_AGE_MS). */
    public static final long PRESS_MAX_AGE_MS = 5000;
    /** Room left between the point pressed and the keyboard's top edge, in dp (as V+). */
    public static final float MARGIN_DP = 24;

    private boolean input;
    private long inputAt;
    // Where the last press was, as a fraction of the stream's height; NaN when it had no point.
    private float pressY = Float.NaN;
    private long pressAt;
    // The focused field: the host's element id (unsigned, above 0) when it sends one, else below
    // 0, counting down on each raise and update, each one another field.
    private long field;
    private long anonymous;
    // The field whose keyboard the user closed, or 0.
    private long dismissed;
    private boolean focused;
    private boolean hostOpened;
    private boolean visible;
    // The point the stream keeps above the keyboard this time it is open; NaN for none.
    private float target = Float.NaN;

    /** The user's own key on the stream. */
    public void onInput(long now) {
        input = true;
        inputAt = now;
    }

    /**
     * The user's own tap or click on the stream, at {@code fractionY} of its height, or NaN when the
     * press moves the PC's pointer instead of pointing (a trackpad finger, a captured mouse).
     */
    public void onPress(float fractionY, long now) {
        onInput(now);
        pressY = fractionY >= 0 && fractionY <= 1 ? fractionY : Float.NaN;
        pressAt = now;
    }

    /**
     * A raise or an update: the focus is in the field with the host's {@code element} id, or in
     * another field when that is 0 (a host before TextInputVersion 2).
     */
    public void onFocus(int element) {
        field = element != 0 ? element & 0xFFFFFFFFL : --anonymous;
        focused = true;
    }

    /** A lower: no field has the focus. Returns whether to hide the keyboard, which the host opened. */
    public boolean onLower() {
        focused = false;
        boolean hide = hostOpened;
        hostOpened = false;
        return hide;
    }

    /** Whether the focused field may open the keyboard now that it is closed. */
    public boolean mayOpen(long now) {
        return focused && dismissed != field && followsInput(now);
    }

    /** Whether the user's own input came at most {@link #INPUT_WINDOW_MS} before now. */
    public boolean followsInput(long now) {
        return input && now - inputAt >= 0 && now - inputAt <= INPUT_WINDOW_MS;
    }

    /** Whether the user closed the keyboard on the focused field. */
    public boolean dismissedHere() {
        return focused && dismissed == field;
    }

    /** The host's field opened the keyboard, so its lower may close it again. */
    public void onOpened() {
        hostOpened = true;
    }

    public boolean hostOpened() {
        return hostOpened;
    }

    /**
     * The keyboard showed or hid. Showing locks the point the stream keeps above it, if a press
     * came shortly before into a host field; hiding while a field has the focus is the user closing
     * it, which keeps it closed for that field and makes a keyboard opened again by hand theirs.
     * Returns whether this changed anything.
     */
    public boolean onVisibility(boolean shown, long now) {
        if (shown == visible) return false;
        visible = shown;
        if (shown) {
            target = focused && !Float.isNaN(pressY) && now - pressAt >= 0 && now - pressAt <= PRESS_MAX_AGE_MS
                    ? pressY : Float.NaN;
        } else {
            if (focused) dismissed = field;
            hostOpened = false;
            target = Float.NaN;
        }
        return true;
    }

    public boolean visible() {
        return visible;
    }

    /**
     * How far to move the stream up (negative) or 0, in the stream's parent: {@code parentHeight}
     * tall, the keyboard covering {@code imeBottom} of its bottom, the stream laid out from
     * {@code streamTop} for {@code streamHeight}. A floating keyboard covers no bottom: no move.
     */
    public float pan(int parentHeight, int imeBottom, float streamTop, float streamHeight, float margin) {
        if (!visible || Float.isNaN(target) || parentHeight <= 0 || imeBottom <= 0 || streamHeight <= 0) return 0;
        return offset(streamTop + target * streamHeight, parentHeight - Math.min(imeBottom, parentHeight), margin);
    }

    /** V+'s viewport offset: up just far enough that {@code focus} sits {@code margin} above {@code visibleBottom}. */
    public static float offset(float focus, float visibleBottom, float margin) {
        return Math.min(0, visibleBottom - margin - focus);
    }

    /** A new connection: no field, no input, no target. Whether the keyboard shows is kept. */
    public void reset() {
        input = false;
        pressY = Float.NaN;
        field = 0;
        anonymous = 0;
        dismissed = 0;
        focused = false;
        hostOpened = false;
        target = Float.NaN;
    }
}
