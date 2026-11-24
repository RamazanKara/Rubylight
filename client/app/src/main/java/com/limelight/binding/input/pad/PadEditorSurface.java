package com.limelight.binding.input.pad;

import android.annotation.SuppressLint;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.view.MotionEvent;
import android.view.View;

import java.util.Arrays;

/**
 * The full-screen editing surface behind {@link PadEditor}'s glass panel: draws the dimmed stream, grid, controls with
 * their selection outlines and alignment guides, and handles tap-to-pick, drag-with-alignment, pinch-resize and corner
 * handle sizing.
 */
@SuppressLint("ViewConstructor") // Created by PadEditor.
final class PadEditorSurface extends View {
    private final PadEditor editor;
    private final PadPainter painter;
    private final PadLayout draft;
    private final float density;
    private final Paint scrim = new Paint(), grid = new Paint(), guide = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final float[] box = new float[4], other = new float[4];
    private final PadPainter.State idle = new PadPainter.State(), pointerIdle = new PadPainter.State();
    private float offsetX, offsetY, pinchDistance, pinchWidth, pinchHeight, anchorX, anchorY;
    private boolean dragging, pinching;
    /** The corner handle being dragged ({@link #corner}'s bits), or -1. */
    private int resizing = -1;
    /** The other controls' lines along one axis while a control is dragged ({@link #alignNudge}). */
    private float[] lines = new float[96];
    /** The lines the dragged control shares with others now: x of the upright guides, y of the level ones. */
    private final float[] guideX = new float[3], guideY = new float[3];
    private int guidesX, guidesY;

    PadEditorSurface(Context context, PadEditor editor, PadPainter painter, PadLayout draft, float density) {
        super(context);
        this.editor = editor;
        this.painter = painter;
        this.draft = draft;
        this.density = density;
        scrim.setColor(0x99000000);
        grid.setColor(0x14FFFFFF);
        grid.setStrokeWidth(Math.max(1, density));
        guide.setColor(painter.accent());
        guide.setStrokeWidth(Math.max(1, density));
    }

    /** How many alignment guides the surface shows now, for tests. */
    int guides() {
        return guidesX + guidesY;
    }

    @Override
    protected void onDraw(Canvas canvas) {
        float width = getWidth(), height = getHeight();
        canvas.drawRect(0, 0, width, height, scrim);
        float step = PadEditor.GRID_DP * 3 * density;
        for (float x = step; x < width; x += step) canvas.drawLine(x, 0, x, height, grid);
        for (float y = step; y < height; y += step) canvas.drawLine(0, y, width, y, grid);
        float base = editor.baseOpacity();
        PadLayout.Control selected = editor.selected();
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (!editor.onSurface(control)) continue;
            PadLayout.Spec spec = draft.spec(control);
            draft.bounds(control, width, height, density, box);
            // Show the real opacity, but never so faint that a control cannot be found.
            float alpha = Math.max(.35f, base * spec.opacity / 100f);
            int saved = canvas.saveLayerAlpha(box[0], box[1], box[0] + box[2], box[1] + box[3],
                    Math.round(alpha * 255));
            painter.draw(canvas, control, box[0], box[1], box[2], box[3], previewState(control));
            canvas.restoreToCount(saved);
            painter.drawOutline(canvas, box[0], box[1], box[2], box[3], control == selected);
        }
        for (int i = 0; i < guidesX; i++) canvas.drawLine(guideX[i], 0, guideX[i], height, guide);
        for (int i = 0; i < guidesY; i++) canvas.drawLine(0, guideY[i], width, guideY[i], guide);
    }

    /** What a control shows at rest here: the pointer touchpad its mode's word, as it does in play. */
    PadPainter.State previewState(PadLayout.Control control) {
        if (control != PadLayout.Control.POINTER_PAD) return idle;
        pointerIdle.label = painter.pointerLabel(draft.pointerMode() == PadLayout.PointerMode.CAMERA);
        return pointerIdle;
    }

    private PadLayout.Control hit(float x, float y) {
        PadLayout.Control[] controls = PadLayout.Control.values();
        float slop = 8 * density;
        // The last drawn is on top, so it wins.
        for (int i = controls.length - 1; i >= 0; i--) {
            if (!editor.onSurface(controls[i])) continue;
            draft.bounds(controls[i], getWidth(), getHeight(), density, box);
            if (x >= box[0] - slop && x <= box[0] + box[2] + slop && y >= box[1] - slop && y <= box[1] + box[3] + slop) {
                return controls[i];
            }
        }
        return null;
    }

    /**
     * The picked control's corner handle under ({@code x}, {@code y}), as {@link PadEditor#RIGHT_CORNER} and
     * {@link PadEditor#BOTTOM_CORNER} bits, or -1. Each handle takes a 48 dp touch around the corner it is drawn on;
     * inside the control only a corner's quarter counts, so a small control's middle still moves it.
     */
    private int corner(float x, float y) {
        PadLayout.Control selected = editor.selected();
        if (selected == null || !editor.onSurface(selected)) return -1;
        draft.bounds(selected, getWidth(), getHeight(), density, box);
        float reach = PadLayout.MIN_TOUCH_DP / 2 * density, outline = PadPainter.OUTLINE_DP * density;
        float insideX = Math.min(reach, box[2] / 4), insideY = Math.min(reach, box[3] / 4);
        boolean inside = x > box[0] && x < box[0] + box[2] && y > box[1] && y < box[1] + box[3];
        int best = -1;
        float bestDistance = Float.MAX_VALUE;
        for (int corner = 0; corner < 4; corner++) {
            boolean right = (corner & PadEditor.RIGHT_CORNER) != 0, bottom = (corner & PadEditor.BOTTOM_CORNER) != 0;
            float cornerX = right ? box[0] + box[2] : box[0], cornerY = bottom ? box[1] + box[3] : box[1];
            float handleX = cornerX + (right ? outline : -outline), handleY = cornerY + (bottom ? outline : -outline);
            if (Math.abs(x - handleX) > reach || Math.abs(y - handleY) > reach) continue;
            if (inside && (Math.abs(x - cornerX) > insideX || Math.abs(y - cornerY) > insideY)) continue;
            float distance = (float) Math.hypot(x - handleX, y - handleY);
            if (distance < bestDistance) {
                best = corner;
                bestDistance = distance;
            }
        }
        return best;
    }

    @SuppressLint("ClickableViewAccessibility") // Editing gestures; the panel's buttons are the accessible path.
    @Override
    public boolean onTouchEvent(MotionEvent event) {
        PadLayout.Control selected = editor.selected();
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN: {
                pinching = false;
                guidesX = guidesY = 0;
                // A corner handle of the picked control wins over whatever lies under it.
                resizing = corner(event.getX(), event.getY());
                if (resizing >= 0) {
                    dragging = false;
                    draft.bounds(selected, getWidth(), getHeight(), density, box);
                    boolean right = (resizing & PadEditor.RIGHT_CORNER) != 0, bottom = (resizing & PadEditor.BOTTOM_CORNER) != 0;
                    // The dragged corner follows the finger from where it took hold; the opposite one stays.
                    offsetX = (right ? box[0] + box[2] : box[0]) - event.getX();
                    offsetY = (bottom ? box[1] + box[3] : box[1]) - event.getY();
                    anchorX = right ? box[0] : box[0] + box[2];
                    anchorY = bottom ? box[1] : box[1] + box[3];
                    break;
                }
                PadLayout.Control control = hit(event.getX(), event.getY());
                editor.select(control);
                dragging = control != null;
                if (dragging) {
                    draft.bounds(control, getWidth(), getHeight(), density, box);
                    offsetX = box[0] + box[2] / 2 - event.getX();
                    offsetY = box[1] + box[3] / 2 - event.getY();
                }
                break;
            }
            case MotionEvent.ACTION_POINTER_DOWN:
                if (selected != null && event.getPointerCount() == 2) {
                    pinching = true;
                    dragging = false;
                    resizing = -1;
                    guidesX = guidesY = 0;
                    pinchDistance = distance(event);
                    pinchWidth = draft.spec(selected).widthScale;
                    pinchHeight = draft.spec(selected).heightScale;
                }
                break;
            case MotionEvent.ACTION_MOVE:
                if (pinching && event.getPointerCount() >= 2 && pinchDistance > 0) {
                    draft.scaleFrom(selected, pinchWidth, pinchHeight, distance(event) / pinchDistance, getWidth(),
                            getHeight(), density);
                    editor.changed();
                } else if (resizing >= 0 && selected != null) {
                    resize(selected, event.getX() + offsetX, event.getY() + offsetY);
                } else if (dragging && selected != null) {
                    drag(selected, event.getX() + offsetX, event.getY() + offsetY);
                }
                break;
            case MotionEvent.ACTION_POINTER_UP:
                // The pinch ends; the remaining finger does not jump the control.
                pinching = false;
                break;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_CANCEL:
                dragging = false;
                pinching = false;
                resizing = -1;
                guidesX = guidesY = 0;
                invalidate();
                editor.keepPanelClear();
                editor.updatePanel();
                break;
            default:
                break;
        }
        return true;
    }

    /**
     * Moves the dragged control's centre to ({@code x}, {@code y}): onto another control's centre or edge line when
     * one is within half a grid step, else onto the grid; then shows the lines it shares with others.
     */
    private void drag(PadLayout.Control selected, float x, float y) {
        float width = getWidth(), height = getHeight(), grid = PadEditor.GRID_DP * density, tolerance = grid / 2;
        draft.bounds(selected, width, height, density, box);
        float w = box[2], h = box[3];
        int count = otherLines(selected, true);
        float nudge = alignNudge(x - w / 2, x, x + w / 2, lines, count, tolerance);
        x = Float.isNaN(nudge) ? Math.round(x / grid) * grid : x + nudge;
        count = otherLines(selected, false);
        nudge = alignNudge(y - h / 2, y, y + h / 2, lines, count, tolerance);
        y = Float.isNaN(nudge) ? Math.round(y / grid) * grid : y + nudge;
        draft.moveTo(selected, x, y, width, height, density);
        // Where it landed, kept on the surface, decides the guides.
        draft.bounds(selected, width, height, density, box);
        float left = box[0], top = box[1], right = box[0] + box[2], bottom = box[1] + box[3];
        count = otherLines(selected, true);
        guidesX = alignedLines(left, (left + right) / 2, right, lines, count, guideX);
        count = otherLines(selected, false);
        guidesY = alignedLines(top, (top + bottom) / 2, bottom, lines, count, guideY);
        invalidate();
    }

    /** The other shown controls' start, centre and end along x ({@code across}) or y, into {@link #lines}. */
    private int otherLines(PadLayout.Control selected, boolean across) {
        int count = 0;
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (control == selected || !editor.onSurface(control)) continue;
            draft.bounds(control, getWidth(), getHeight(), density, other);
            if (lines.length < count + 3) lines = Arrays.copyOf(lines, lines.length * 2);
            float start = across ? other[0] : other[1], size = across ? other[2] : other[3];
            lines[count++] = start;
            lines[count++] = start + size / 2;
            lines[count++] = start + size;
        }
        return count;
    }

    /**
     * Puts the dragged corner at ({@code x}, {@code y}), on the grid or flush with the surface's edge, and sizes
     * the picked control's width and height between it and the opposite corner, which stays: never smaller than
     * a full touch, never past the surface.
     */
    private void resize(PadLayout.Control selected, float x, float y) {
        float width = getWidth(), height = getHeight();
        float u = PadLayout.unit(width, height, density);
        float leastWide = PadLayout.width(selected, u, density, PadLayout.MIN_SCALE);
        float leastHigh = PadLayout.height(selected, u, density, PadLayout.MIN_SCALE);
        x = snapEdge(x, width);
        y = snapEdge(y, height);
        float left, right, top, bottom;
        if ((resizing & PadEditor.RIGHT_CORNER) != 0) {
            left = anchorX;
            right = Math.max(anchorX + leastWide, Math.min(width, x));
        } else {
            right = anchorX;
            left = Math.min(anchorX - leastWide, Math.max(0, x));
        }
        if ((resizing & PadEditor.BOTTOM_CORNER) != 0) {
            top = anchorY;
            bottom = Math.max(anchorY + leastHigh, Math.min(height, y));
        } else {
            bottom = anchorY;
            top = Math.min(anchorY - leastHigh, Math.max(0, y));
        }
        draft.setBounds(selected, left, top, right - left, bottom - top, width, height, density);
        editor.changed();
    }

    /** A dragged edge lands on the grid, or flush with the surface's edge when that is nearer. */
    private float snapEdge(float position, float size) {
        float grid = PadEditor.GRID_DP * density;
        float snapped = Math.round(position / grid) * grid;
        return Math.abs(size - position) < Math.abs(snapped - position) ? size : snapped;
    }

    private float distance(MotionEvent event) {
        return (float) Math.hypot(event.getX(0) - event.getX(1), event.getY(0) - event.getY(1));
    }

    /**
     * The nudge that lines a dragged box's start, centre or end up with another control's when one is within
     * {@code tolerance}, the nearest winning; NaN when none is near. {@code lines} holds each other control's start,
     * centre and end along the same axis, {@code count} values in all. Edges meet edges, on either side, and centres
     * meet centres.
     */
    static float alignNudge(float start, float centre, float end, float[] lines, int count, float tolerance) {
        float best = Float.NaN;
        for (int i = 0; i + 2 < count; i += 3) {
            float otherStart = lines[i], otherCentre = lines[i + 1], otherEnd = lines[i + 2];
            best = nearer(best, otherCentre - centre, tolerance);
            best = nearer(best, otherStart - start, tolerance);
            best = nearer(best, otherEnd - end, tolerance);
            best = nearer(best, otherStart - end, tolerance);
            best = nearer(best, otherEnd - start, tolerance);
        }
        return best;
    }

    private static float nearer(float best, float candidate, float tolerance) {
        if (Math.abs(candidate) > tolerance) return best;
        return Float.isNaN(best) || Math.abs(candidate) < Math.abs(best) ? candidate : best;
    }

    /**
     * Which of the other controls' {@code lines} (as {@link #alignNudge} takes them) the box's start, centre or end lies
     * on, to half a pixel: edges on edges, centres on centres. Each line goes once into {@code out}, as far as it
     * reaches; returns how many.
     */
    static int alignedLines(float start, float centre, float end, float[] lines, int count, float[] out) {
        int found = 0;
        for (int mine = 0; mine < 3 && found < out.length; mine++) {
            float line = mine == 0 ? start : mine == 1 ? centre : end;
            for (int i = 0; i < count; i++) {
                if ((i % 3 == 1) != (mine == 1) || Math.abs(lines[i] - line) > .5f) continue;
                boolean known = false;
                for (int k = 0; k < found; k++) known |= Math.abs(out[k] - line) <= .5f;
                if (!known) out[found++] = line;
                break;
            }
        }
        return found;
    }
}
