package com.limelight.binding.input.pad;

/**
 * Cluster split, join, and face-button arrangement geometry for {@link PadLayout}: splitting the
 * face cluster or D-pad into four independent controls, clearing neighbours, and computing the
 * enclosing cluster box and arrangement when rejoining.
 */
final class PadLayoutClusters {
    /** How much nearer one arrangement must fit than the other to decide, in squared fractions of the span. */
    private static final float ARRANGEMENT_MARGIN = .01f;
    /** How much larger than the members' a joined cluster's buttons may be drawn. */
    private static final float MAX_JOINED_GROWTH = 1.5f;

    private PadLayoutClusters() {
    }

    /**
     * Splits the face cluster or the D-pad into its four members: the cluster hides, and each member shows in its
     * button's direction from the cluster's centre on this {@code width} x {@code height} surface ({@link #memberBox}),
     * at that button's size (never below a full touch) and the cluster's opacity. {@link #join} undoes it.
     */
    static void split(PadLayout layout, PadLayout.Control cluster, float width, float height, float density) {
        PadLayout.Control[] members = PadLayout.members(cluster);
        if (members.length == 0) return;
        if (joinedFromMembers(layout, cluster, width, height, density)) {
            // Joined from these members and not moved or resized since: they come back as they stood, as faint as
            // the cluster, unless a control moved onto one of their places meanwhile.
            float[][] stood = new float[members.length][];
            for (int i = 0; i < members.length; i++) stood[i] = layout.bounds(members[i], width, height, density, new float[4]);
            if (clear(stood, 0, 0, neighbours(layout, cluster, width, height, density), width, height)) {
                for (PadLayout.Control member : members) {
                    layout.spec(member).hidden = false;
                    layout.spec(member).opacity = layout.spec(cluster).opacity;
                }
                layout.spec(cluster).hidden = true;
                return;
            }
        }
        float[][] boxes = splitBoxes(layout, cluster, width, height, density);
        // Each place gets the button the cluster shows there, so a Nintendo arrangement stays one.
        int[] order = order(layout, cluster);
        for (int i = 0; i < members.length; i++) {
            PadLayout.Control control = members[order[i]];
            layout.setBounds(control, boxes[i][0], boxes[i][1], boxes[i][2], boxes[i][3], width, height, density);
            layout.spec(control).opacity = layout.spec(cluster).opacity;
            layout.spec(control).hidden = false;
        }
        layout.spec(cluster).hidden = true;
    }

    /** Where {@link #split} puts the members' places around the cluster's own box, in {@link PadInput#FACE_CENTERS} order. */
    private static float[][] splitBoxes(PadLayout layout, PadLayout.Control cluster, float width, float height, float density) {
        float[] box = layout.bounds(cluster, width, height, density, new float[4]);
        float[][] boxes = memberBoxes(cluster, box[0], box[1], box[2], box[3], width, height, density);
        clearOfNeighbours(layout, cluster, boxes, width, height, density);
        return boxes;
    }

    /** The member at each place of the cluster: the face cluster's arrangement, the D-pad's fixed one. */
    private static int[] order(PadLayout layout, PadLayout.Control cluster) {
        return cluster == PadLayout.Control.FACE ? PadInput.faceOrder(layout.facesSwapped()) : PadInput.FACE_STANDARD;
    }

    /**
     * Moves a split cluster's member boxes together off the other controls shown, where spreading them carried one
     * over a neighbour: by the least move that clears every neighbour and keeps them on this {@code width} x
     * {@code height} surface. Where no single move does, they stay. The Steam Controller controls, which a setting
     * shows, do not move them, as they do not move the WASD stick ({@link PadLayoutDefaults#clearOfDefaults}).
     */
    private static void clearOfNeighbours(PadLayout layout, PadLayout.Control cluster, float[][] boxes, float width,
                                          float height, float density) {
        java.util.List<float[]> others = neighbours(layout, cluster, width, height, density);
        if (clear(boxes, 0, 0, others, width, height)) return;
        // Each move puts an overlapped neighbour's edge two pixels past the member's, against rounding.
        float bestX = 0, bestY = 0, best = Float.MAX_VALUE;
        for (float[] box : boxes) {
            for (float[] other : others) {
                if (!overlaps(box, 0, 0, other)) continue;
                float[][] moves = {{other[0] + other[2] - box[0] + 2, 0}, {other[0] - box[0] - box[2] - 2, 0},
                        {0, other[1] + other[3] - box[1] + 2}, {0, other[1] - box[1] - box[3] - 2}};
                for (float[] move : moves) {
                    float length = Math.abs(move[0]) + Math.abs(move[1]);
                    if (length < best && clear(boxes, move[0], move[1], others, width, height)) {
                        best = length;
                        bestX = move[0];
                        bestY = move[1];
                    }
                }
            }
        }
        if (best == Float.MAX_VALUE) return;
        for (float[] box : boxes) {
            box[0] += bestX;
            box[1] += bestY;
        }
    }

    /** The boxes of the controls shown that a split cluster's members keep off: all but its own and Steam's. */
    private static java.util.List<float[]> neighbours(PadLayout layout, PadLayout.Control cluster, float width,
                                                      float height, float density) {
        java.util.List<float[]> others = new java.util.ArrayList<>();
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (control == cluster || control.cluster() == cluster || control.steam || !layout.shown(control)) continue;
            others.add(layout.bounds(control, width, height, density, new float[4]));
        }
        return others;
    }

    /** Whether {@code boxes} moved by {@code dx}, {@code dy} stay on the surface and off every one of {@code others}. */
    private static boolean clear(float[][] boxes, float dx, float dy, java.util.List<float[]> others, float width,
                                 float height) {
        for (float[] box : boxes) {
            if (box[0] + dx < 0 || box[1] + dy < 0 || box[0] + dx + box[2] > width || box[1] + dy + box[3] > height) {
                return false;
            }
            for (float[] other : others) {
                if (overlaps(box, dx, dy, other)) return false;
            }
        }
        return true;
    }

    private static boolean overlaps(float[] box, float dx, float dy, float[] other) {
        return box[0] + dx < other[0] + other[2] && other[0] < box[0] + dx + box[2]
                && box[1] + dy < other[1] + other[3] && other[1] < box[1] + dy + box[3];
    }

    /**
     * Joins a split cluster again where its members stand on this {@code width} x {@code height} surface, and they
     * hide. Members still where {@link #split} put them give the cluster back as it was. Moved, or never split from
     * it (a layout carried over), the cluster gets the place and size from which {@link #split} would lay them out
     * as they stand ({@link #clusterFor}), so a Join and a Split keep their size however often they follow each
     * other; one member alone is enough. The face cluster keeps the arrangement they stood in, the standard one or the
     * Nintendo one ({@link PadLayout#facesSwapped}); where the members shown cannot tell, it keeps the one it had.
     */
    static void join(PadLayout layout, PadLayout.Control cluster, float width, float height, float density) {
        PadLayout.Control[] members = PadLayout.members(cluster);
        float[][] boxes = new float[members.length][];
        boolean any = false;
        for (int i = 0; i < members.length; i++) {
            if (!layout.shown(members[i])) continue;
            boxes[i] = layout.bounds(members[i], width, height, density, new float[4]);
            any = true;
        }
        if (any && width > 0 && height > 0) {
            if (cluster == PadLayout.Control.FACE) {
                layout.setFacesSwapped(arrangement(centers(boxes), layout.facesSwapped()));
            }
            if (!whereSplitPutThem(layout, cluster, boxes, width, height, density)) {
                float[] box = clusterFor(layout, cluster, boxes, layout.bounds(cluster, width, height, density, new float[4]),
                        width, height, density);
                layout.setBounds(cluster, box[0], box[1], box[2], box[3], width, height, density);
            }
        }
        for (PadLayout.Control member : members) layout.spec(member).hidden = true;
        layout.spec(cluster).hidden = false;
    }

    /**
     * Takes the face cluster's arrangement from where its members stand, while it is split or before it shows again
     * with all four hidden: from the members shown, or with none shown, from every placed one. A preset and Add then
     * show the cluster as the members stood, not in an arrangement only Join set. Unchanged where they cannot tell.
     */
    static void takeFaceArrangement(PadLayout layout) {
        PadLayout.Control[] members = PadLayout.members(PadLayout.Control.FACE);
        boolean anyShown = false;
        for (PadLayout.Control member : members) anyShown |= layout.shown(member);
        float[][] centers = new float[members.length][];
        for (int i = 0; i < members.length; i++) {
            PadLayout.Spec spec = layout.spec(members[i]);
            if (spec.placed() && (!anyShown || layout.shown(members[i]))) centers[i] = new float[] {spec.cx, spec.cy};
        }
        layout.setFacesSwapped(arrangement(centers, layout.facesSwapped()));
    }

    private static float[][] centers(float[][] boxes) {
        float[][] centers = new float[boxes.length][];
        for (int i = 0; i < boxes.length; i++) {
            if (boxes[i] != null) centers[i] = new float[] {boxes[i][0] + boxes[i][2] / 2, boxes[i][1] + boxes[i][3] / 2};
        }
        return centers;
    }

    /**
     * Whether face members centred at {@code centers} (null where left out) stand in the Nintendo arrangement rather
     * than the standard one, judged in the box they span; {@code current} where they cannot tell. A and X alone, or B
     * and Y alone, stand alike in both, as does one member alone.
     */
    static boolean arrangement(float[][] centers, boolean current) {
        int count = 0;
        boolean ax = true, by = true;
        float left = Float.MAX_VALUE, top = Float.MAX_VALUE, right = -Float.MAX_VALUE, bottom = -Float.MAX_VALUE;
        for (int i = 0; i < centers.length; i++) {
            if (centers[i] == null) continue;
            count++;
            ax &= i == 0 || i == 2;
            by &= i == 1 || i == 3;
            left = Math.min(left, centers[i][0]);
            right = Math.max(right, centers[i][0]);
            top = Math.min(top, centers[i][1]);
            bottom = Math.max(bottom, centers[i][1]);
        }
        if (count < 2 || ax || by) return current;
        // The centres span the buttons' places, from .22 to .78 of a cluster where they spread both ways.
        float spanX = Math.max(right - left, 1e-6f), spanY = Math.max(bottom - top, 1e-6f);
        float standard = 0, swapped = 0;
        for (int i = 0; i < centers.length; i++) {
            if (centers[i] == null) continue;
            float x = (centers[i][0] - left) / spanX, y = (centers[i][1] - top) / spanY;
            standard += distance(x, y, spanned(PadInput.FACE_CENTERS[i]));
            swapped += distance(x, y, spanned(PadInput.FACE_CENTERS[PadInput.FACE_SWAPPED[i]]));
        }
        if (Math.abs(standard - swapped) < ARRANGEMENT_MARGIN) return current;
        return swapped < standard;
    }

    /** A face button's place in the box the four centres span: 0 to 1 where {@link PadInput#FACE_CENTERS} has .22 to .78. */
    private static float[] spanned(float[] center) {
        return new float[] {(center[0] - .22f) / .56f, (center[1] - .22f) / .56f};
    }

    /**
     * The cluster box ({left, top, width, height}) for members standing in {@code boxes} (null where hidden) on a
     * {@code surfaceWidth} x {@code surfaceHeight} surface. The face cluster: {@link #faceClusterFor}. The D-pad inverts
     * {@link #memberBox}: the arms' reach gives each side (the shorter reach the shorter side), else their thickness
     * the shorter side, and the middle is where the arms' offsets from it put it. Arms at a full touch, the least they
     * get, tell only that the cluster was no larger: the cluster's own size ({@code now}) stays where it fits that.
     */
    private static float[] clusterFor(PadLayout layout, PadLayout.Control cluster, float[][] boxes, float[] now,
                                      float surfaceWidth, float surfaceHeight, float density) {
        if (cluster == PadLayout.Control.FACE) {
            return faceClusterFor(layout, boxes, now, surfaceWidth, surfaceHeight, density);
        }
        float touch = PadLayout.MIN_TOUCH_DP * density;
        float thick = 0, alongV = 0, alongH = 0;
        int countT = 0;
        for (int i = 0; i < boxes.length; i++) {
            if (boxes[i] == null) continue;
            boolean vertical = i < 2;
            thick += vertical ? boxes[i][2] : boxes[i][3];
            countT++;
            if (vertical) alongV = Math.max(alongV, boxes[i][3]);
            else alongH = Math.max(alongH, boxes[i][2]);
        }
        thick /= countT;
        // Square, an arm reaches (1 - DPAD_ARM) / 2 + DPAD_CORNER of the side; the shorter reach is the shorter side's.
        float reachSquare = (1 - PadInput.DPAD_ARM) / 2 + PadInput.DPAD_CORNER;
        boolean byV = alongV > touch + .5f, byH = alongH > touch + .5f;
        float shorter;
        if (byV || byH) {
            shorter = (byV && byH ? Math.min(alongV, alongH) : byV ? alongV : alongH) / reachSquare;
        } else if (thick > touch + .5f) {
            shorter = thick / PadInput.DPAD_ARM;
        } else {
            float nowShorter = Math.min(now[2], now[3]);
            shorter = nowShorter * reachSquare <= touch + .5f ? nowShorter : touch / reachSquare;
        }
        float arm = PadInput.DPAD_ARM * shorter, corner = PadInput.DPAD_CORNER * shorter;
        // Each side from its own arms' reach, so the shorter one is the shorter side and the other no shorter.
        float h = byV ? Math.max(shorter, 2 * (alongV - corner) + arm) : shorter;
        float w = byH ? Math.max(shorter, 2 * (alongH - corner) + arm) : shorter;
        // The middle: each arm shown, less its offset from the middle of a D-pad this size.
        float x = 0, y = 0;
        int count = 0;
        float[] relative = new float[4];
        PadLayout.Control[] members = PadLayout.members(cluster);
        for (int i = 0; i < boxes.length; i++) {
            if (boxes[i] == null) continue;
            memberBox(members[i], 0, 0, w, h, density, relative);
            x += centerX(boxes[i]) - (relative[0] + relative[2] / 2 - w / 2);
            y += centerY(boxes[i]) - (relative[1] + relative[3] / 2 - h / 2);
            count++;
        }
        x /= count;
        y /= count;
        return new float[] {x - w / 2, y - h / 2, w, h};
    }

    /**
     * Whether a face pair half {@code spread} apart sets its side: in its order, at least three quarters as far apart
     * as Split spreads members of their size ({@code least}), and not exactly that far, which only Split gives.
     */
    private static boolean drawn(float spread, float least) {
        return !Float.isNaN(spread) && spread >= .75f * least && Math.abs(spread - least) > 1.5f;
    }

    /**
     * The face cluster box for members standing in {@code boxes}: each button of the cluster drawn where its member
     * stands. Each side comes from its own pair's spacing, as the cluster draws its buttons .28 of the side from the
     * middle, so a symmetric diamond gives a square cluster; its buttons may come out larger than the members were.
     * A pair exactly as far apart as {@link #split} spreads members of their size came from a split, so that side is
     * the one their size gives ({@link #memberBox}), and a Split gives the same members back. A side no pair tells
     * is the one their size gives too, and members at a full touch keep the cluster's own size ({@code now}) where it
     * fits. A side the surface has no room for around the middle narrows to fit; where that would go below the size
     * the members' buttons need, the cluster is the square that size gives.
     */
    private static float[] faceClusterFor(PadLayout layout, float[][] boxes, float[] now, float surfaceWidth,
                                          float surfaceHeight, float density) {
        float touch = PadLayout.MIN_TOUCH_DP * density, twoRadius = 2 * PadInput.FACE_RADIUS;
        float reach = PadInput.FACE_CENTERS[1][0] - .5f;
        int[] order = order(layout, PadLayout.Control.FACE);
        float size = 0;
        int count = 0;
        float[][] places = new float[boxes.length][];
        for (int i = 0; i < boxes.length; i++) {
            if (boxes[i] == null) continue;
            size += (boxes[i][2] + boxes[i][3]) / 2;
            count++;
            places[order[i]] = boxes[i];
        }
        size = Math.max(size / count, touch);
        float shorter = size / twoRadius, least = size + 2;
        float nowShorter = Math.min(now[2], now[3]);
        if (size <= touch + .5f && twoRadius * nowShorter <= touch + .5f) shorter = nowShorter;
        // Places: 0 bottom, 1 right, 2 left, 3 top. Each pair's half spacing, NaN without both.
        float spreadX = places[1] != null && places[2] != null ? (centerX(places[1]) - centerX(places[2])) / 2 : Float.NaN;
        float spreadY = places[0] != null && places[3] != null ? (centerY(places[0]) - centerY(places[3])) / 2 : Float.NaN;
        // Heaped or mirrored, a pair tells nothing a cluster could draw; a little closer than Split spreads them, it
        // still does.
        boolean drawnX = drawn(spreadX, least), drawnY = drawn(spreadY, least);
        float w = Math.max(shorter, drawnX ? spreadX / reach : fitting(now[2], shorter, least / reach));
        float h = Math.max(shorter, drawnY ? spreadY / reach : fitting(now[3], shorter, least / reach));
        // Members spread far apart would give buttons far larger than theirs: the square their size gives instead,
        // centred on them.
        if (twoRadius * Math.min(w, h) > MAX_JOINED_GROWTH * size) w = h = shorter;
        // The middle, along each axis: between its pair, else one of them less its offset, else the other pair's line.
        float x = middle(places[1], places[2], places[0], places[3], true, w, h, density);
        float y = middle(places[0], places[3], places[1], places[2], false, w, h, density);
        float roomX = 2 * Math.min(x, surfaceWidth - x), roomY = 2 * Math.min(y, surfaceHeight - y);
        if (w > roomX || h > roomY) {
            w = Math.min(w, roomX);
            h = Math.min(h, roomY);
            if (Math.min(w, h) < shorter) w = h = shorter;
        }
        return new float[] {x - w / 2, y - h / 2, w, h};
    }

    /**
     * The face cluster's middle along one axis ({@code horizontal}: x): between {@code plus} (right or bottom) and
     * {@code minus}; from one of them less the offset Split gives a cluster {@code w} x {@code h}; else on the line of
     * the other pair, which stands on the middle along this axis.
     */
    private static float middle(float[] plus, float[] minus, float[] across1, float[] across2, boolean horizontal,
                                float w, float h, float density) {
        if (plus != null && minus != null) return horizontal ? (centerX(plus) + centerX(minus)) / 2 :
                (centerY(plus) + centerY(minus)) / 2;
        if (plus != null || minus != null) {
            float button = Math.max(2 * PadInput.FACE_RADIUS * Math.min(w, h), PadLayout.MIN_TOUCH_DP * density);
            float offset = Math.max((PadInput.FACE_CENTERS[1][0] - .5f) * (horizontal ? w : h), button + 2);
            float[] one = plus != null ? plus : minus;
            float at = horizontal ? centerX(one) : centerY(one);
            return plus != null ? at - offset : at + offset;
        }
        float sum = 0;
        int n = 0;
        for (float[] box : new float[][] {across1, across2}) {
            if (box == null) continue;
            sum += horizontal ? centerX(box) : centerY(box);
            n++;
        }
        return sum / n;
    }

    /** {@code side} where it lies from {@code least} to {@code most}, else {@code least}. */
    private static float fitting(float side, float least, float most) {
        return side >= least - .5f && side <= most + .5f ? side : least;
    }

    private static float centerX(float[] box) {
        return box[0] + box[2] / 2;
    }

    private static float centerY(float[] box) {
        return box[1] + box[3] / 2;
    }

    /**
     * Whether the cluster is the one {@link #join} made from its four members as they still stand: all four placed,
     * and the cluster they give is the cluster as it stands, to the pixel.
     */
    private static boolean joinedFromMembers(PadLayout layout, PadLayout.Control cluster, float width, float height,
                                             float density) {
        if (width <= 0 || height <= 0 || !layout.spec(cluster).placed()) return false;
        PadLayout.Control[] members = PadLayout.members(cluster);
        float[][] boxes = new float[members.length][];
        for (int i = 0; i < members.length; i++) {
            if (!layout.spec(members[i]).placed()) return false;
            boxes[i] = layout.bounds(members[i], width, height, density, new float[4]);
        }
        float[] now = layout.bounds(cluster, width, height, density, new float[4]);
        PadLayout joined = layout.copy();
        float[] box = clusterFor(joined, cluster, boxes, now, width, height, density);
        joined.setBounds(cluster, box[0], box[1], box[2], box[3], width, height, density);
        float[] would = joined.bounds(cluster, width, height, density, new float[4]);
        for (int k = 0; k < 4; k++) {
            if (Math.abs(would[k] - now[k]) > 1) return false;
        }
        return true;
    }

    /** Whether the shown members' {@code boxes} are where splitting the cluster as it stands puts them. */
    private static boolean whereSplitPutThem(PadLayout layout, PadLayout.Control cluster, float[][] boxes, float width,
                                             float height, float density) {
        float[][] places = splitBoxes(layout, cluster, width, height, density);
        int[] order = order(layout, cluster);
        for (int i = 0; i < boxes.length; i++) {
            if (boxes[i] == null) continue;
            // Each order swaps pairs, so the member's own index gives its place.
            float[] place = places[order[i]];
            for (int k = 0; k < 4; k++) {
                if (Math.abs(place[k] - boxes[i][k]) > 2) return false;
            }
        }
        return true;
    }

    private static float distance(float x, float y, float[] center) {
        float dx = x - center[0], dy = y - center[1];
        return dx * dx + dy * dy;
    }

    /**
     * Where a split member sits around its cluster's box ({@code left}, {@code top}, {@code width}, {@code height}),
     * as left, top, width and height in {@code out}: in its button's direction from the cluster's centre, a face
     * button's circle or a D-pad arm as far as its pressed amber reaches, at that size but never below a full touch.
     * The members spread outward from the centre as far as they must so that no two face buttons overlap, since one
     * finger would press both where they did: each sits at least its whole size, and two pixels against rounding, off
     * the centre lines. D-pad neighbours keep only their inner corner in common, in the cluster's proportions to the
     * arm's thickness, so a finger there holds the diagonal; opposite arms never meet, and the middle holds nothing.
     * {@link #memberBoxes} keeps the four on the surface together.
     */
    static void memberBox(PadLayout.Control member, float left, float top, float width, float height, float density,
                          float[] out) {
        float size = Math.min(width, height), touch = PadLayout.MIN_TOUCH_DP * density;
        float middleX = left + width / 2, middleY = top + height / 2;
        float cx = middleX, cy = middleY, w, h;
        if (member.shape == PadLayout.Shape.FACE_BUTTON) {
            w = h = Math.max(2 * PadInput.FACE_RADIUS * size, touch);
            float[] center = PadInput.FACE_CENTERS[member.memberIndex()];
            float dx = (center[0] - .5f) * width, dy = (center[1] - .5f) * height, least = w + 2;
            if (dx != 0) cx += Math.signum(dx) * Math.max(Math.abs(dx), least);
            if (dy != 0) cy += Math.signum(dy) * Math.max(Math.abs(dy), least);
        } else {
            float arm = PadInput.DPAD_ARM * size, corner = PadInput.DPAD_CORNER * size;
            boolean vertical = member == PadLayout.Control.DPAD_UP || member == PadLayout.Control.DPAD_DOWN;
            float length = ((vertical ? height : width) - arm) / 2 + corner;
            float thick = Math.max(arm, touch), along = Math.max(length, touch);
            // The inner end reaches past the crossing arm's edge by the corner, grown with the arm.
            float inner = thick / 2 - corner * thick / Math.max(1e-6f, arm);
            w = vertical ? thick : along;
            h = vertical ? along : thick;
            if (member == PadLayout.Control.DPAD_UP) cy = middleY - inner - along / 2;
            else if (member == PadLayout.Control.DPAD_DOWN) cy = middleY + inner + along / 2;
            else if (member == PadLayout.Control.DPAD_LEFT) cx = middleX - inner - along / 2;
            else cx = middleX + inner + along / 2;
        }
        out[0] = cx - w / 2;
        out[1] = cy - h / 2;
        out[2] = w;
        out[3] = h;
    }

    /**
     * The boxes of {@code cluster}'s members ({@link #memberBox}) in {@link PadLayout#members} order, moved together where
     * spreading them would carry one off this {@code surfaceWidth} x {@code surfaceHeight} surface, so they keep their
     * arrangement.
     */
    static float[][] memberBoxes(PadLayout.Control cluster, float left, float top, float width, float height,
                                 float surfaceWidth, float surfaceHeight, float density) {
        PadLayout.Control[] members = PadLayout.members(cluster);
        float[][] boxes = new float[members.length][4];
        float minX = Float.MAX_VALUE, minY = Float.MAX_VALUE, maxX = -Float.MAX_VALUE, maxY = -Float.MAX_VALUE;
        for (int i = 0; i < members.length; i++) {
            float[] box = boxes[i];
            memberBox(members[i], left, top, width, height, density, box);
            minX = Math.min(minX, box[0]);
            minY = Math.min(minY, box[1]);
            maxX = Math.max(maxX, box[0] + box[2]);
            maxY = Math.max(maxY, box[1] + box[3]);
        }
        float dx = onto(minX, maxX, surfaceWidth), dy = onto(minY, maxY, surfaceHeight);
        for (float[] box : boxes) {
            box[0] += dx;
            box[1] += dy;
        }
        return boxes;
    }

    /** How far to move a span from {@code min} to {@code max} onto 0 to {@code size}: centred if it is longer. */
    private static float onto(float min, float max, float size) {
        if (size <= 0) return 0;
        if (max - min >= size) return (size - min - max) / 2;
        if (min < 0) return -min;
        return max > size ? size - max : 0;
    }
}
