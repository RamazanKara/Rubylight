package com.limelight.binding.input.keyboard;

import java.util.List;

/**
 * Which key a controller or remote has selected. Left and right wrap around the row; up and down
 * stop at the first and last row and land on the key closest to where the selection came from, so
 * going up and straight back down returns to the same key.
 */
public final class KeyboardFocus {
    public enum Direction { UP, DOWN, LEFT, RIGHT }

    private final PcKeyboardLayout layout;
    private int row, column;
    /** Where the selection wants to be horizontally, kept across rows of different shapes. */
    private float anchorX;

    public KeyboardFocus(PcKeyboardLayout layout) {
        this.layout = layout;
        focus("G");
    }

    public PcKey focused() {
        return layout.rows().get(row).get(column);
    }

    public int row() {
        return row;
    }

    public int column() {
        return column;
    }

    /** Select the key with the given id; an unknown id leaves the selection where it is. */
    public boolean focus(String keyId) {
        List<List<PcKey>> rows = layout.rows();
        for (int r = 0; r < rows.size(); r++) {
            for (int c = 0; c < rows.get(r).size(); c++) {
                if (rows.get(r).get(c).id.equals(keyId)) {
                    row = r;
                    column = c;
                    anchorX = focused().centerX();
                    return true;
                }
            }
        }
        return false;
    }

    public void move(Direction direction) {
        List<List<PcKey>> rows = layout.rows();
        switch (direction) {
            case LEFT:
                column = (column + rows.get(row).size() - 1) % rows.get(row).size();
                anchorX = focused().centerX();
                break;
            case RIGHT:
                column = (column + 1) % rows.get(row).size();
                anchorX = focused().centerX();
                break;
            case UP:
                if (row > 0) moveToRow(row - 1);
                break;
            default:
                if (row < rows.size() - 1) moveToRow(row + 1);
                break;
        }
    }

    private void moveToRow(int target) {
        List<PcKey> keys = layout.rows().get(target);
        int best = 0;
        float bestDistance = Float.MAX_VALUE;
        for (int c = 0; c < keys.size(); c++) {
            PcKey key = keys.get(c);
            // A key under the anchor is a perfect hit; otherwise the nearest edge wins.
            float distance = anchorX < key.x ? key.x - anchorX : anchorX > key.x + key.width ? anchorX - key.x - key.width : 0;
            if (distance < bestDistance || (distance == bestDistance && Math.abs(key.centerX() - anchorX) <
                    Math.abs(keys.get(best).centerX() - anchorX))) {
                best = c;
                bestDistance = distance;
            }
        }
        row = target;
        column = best;
    }
}
