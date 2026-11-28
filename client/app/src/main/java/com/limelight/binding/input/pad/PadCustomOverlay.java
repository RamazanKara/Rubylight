package com.limelight.binding.input.pad;

import android.annotation.SuppressLint;
import android.content.Context;
import android.graphics.Rect;
import android.view.Gravity;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.LayoutGamepad;
import com.limelight.binding.input.TouchLayout;
import com.limelight.ui.OverlayPlacement;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Coordinates {@link PadTouchZoneView}s and {@link PadCustomButtonView}s inside {@link ModernController}'s
 * stream frame with strict Z-order (touch zones below {@link PadReachLayer}/{@link PadControlView}
 * and custom buttons above touch zones), zero child additions when empty, edge-to-edge negative
 * margins for touch zones, and safe-inset placement for custom buttons.
 */
final class PadCustomOverlay {
    private static final int ZONE_TOKEN_BASE = 0x10000;
    private static final int BUTTON_TOKEN_BASE = 0x20000;
    private static final int[] MOVE_STICK_SLOTS = {
            PadLayout.Control.WASD.ordinal(),
            PadLayout.Control.FACE.ordinal(),
    };
    private static final int[] LOOK_STICK_SLOTS = {
            PadLayout.Control.DPAD.ordinal(),
            PadLayout.Control.SELECT.ordinal(),
    };

    private final Context context;
    private final FrameLayout frame;
    private final PadSkin70 skin;
    private final PadHeldInputs heldInputs;
    private final LayoutGamepad gamepad;
    private final Runnable gamepadChanged;
    private final PadTouchZoneHandler.AimForwarder aimForwarder = new PadTouchZoneHandler.AimForwarder();
    private final List<PadTouchZoneView> zoneViews = new ArrayList<>();
    private final List<PadCustomButtonView> buttonViews = new ArrayList<>();
    private final float[] box = new float[4];

    private ModernController.PointerSink pointerSink;
    private boolean hapticsEnabled = true;
    private boolean explicitSafeInsets;
    private int safeLeft, safeTop, safeRight, safeBottom;

    private final PadTouchZoneHandler.PointerOut pointerOut = new PadTouchZoneHandler.PointerOut() {
        @Override
        public void move(int dx, int dy) {
            if (pointerSink != null) pointerSink.move(dx, dy);
        }

        @Override
        public void tap(int mouseButton) {
            // Zone views dispatch tap clicks via PadTouchZoneView.onTap; fallback via sink if direct.
            if (pointerSink != null) {
                for (PadTouchZoneView zv : zoneViews) {
                    if (zv.zone.kind.isLook()) {
                        zv.onTap(mouseButton);
                        return;
                    }
                }
            }
        }
    };

    PadCustomOverlay(Context context, FrameLayout frame, PadSkin70 skin, PadHeldInputs heldInputs,
                     LayoutGamepad gamepad, Runnable gamepadChanged) {
        this.context = context;
        this.frame = frame;
        this.skin = skin;
        this.heldInputs = heldInputs;
        this.gamepad = gamepad;
        this.gamepadChanged = gamepadChanged;
        this.safeLeft = frame.getPaddingLeft();
        this.safeTop = frame.getPaddingTop();
        this.safeRight = frame.getPaddingRight();
        this.safeBottom = frame.getPaddingBottom();
    }

    void setPointerSink(ModernController.PointerSink sink) {
        this.pointerSink = sink;
        for (PadTouchZoneView zv : zoneViews) zv.setPointerSink(sink);
        for (PadCustomButtonView bv : buttonViews) bv.setPointerSink(sink);
    }

    void setHaptics(boolean haptics) {
        this.hapticsEnabled = haptics;
        for (PadCustomButtonView bv : buttonViews) bv.setHaptics(haptics);
    }

    boolean setSafeInsets(int left, int top, int right, int bottom) {
        if (explicitSafeInsets && safeLeft == left && safeTop == top
                && safeRight == right && safeBottom == bottom) {
            return false;
        }
        explicitSafeInsets = true;
        safeLeft = Math.max(0, left);
        safeTop = Math.max(0, top);
        safeRight = Math.max(0, right);
        safeBottom = Math.max(0, bottom);
        return true;
    }

    /** Rebuilds overlay views from {@code layout} while guaranteeing 0 views added when empty. */
    @SuppressLint("RtlHardcoded")
    void sync(PadLayout layout, boolean shown, boolean editing, float baseOpacity) {
        removeViews();
        List<PadTouchZone> zones = layout.touchZones();
        List<PadCustomButton> buttons = layout.customButtons();
        if (zones.isEmpty() && buttons.isEmpty()) return;

        configureAimFromZones(zones);
        PadTouchZoneHandler.StickSink aimStickSink = null;
        int moveSlotIdx = 0;
        int lookSlotIdx = 0;
        for (PadTouchZone z : zones) {
            if (z.kind == PadTouchZone.Kind.LOOK_STICK && aimStickSink == null) {
                aimStickSink = stickSink(LOOK_STICK_SLOTS[0]);
            }
        }
        aimForwarder.setSinks(pointerOut, aimStickSink);

        int zoneInsertAt = firstModernChildIndex();
        for (int i = 0; i < zones.size(); i++) {
            PadTouchZone z = zones.get(i);
            PadTouchZoneHandler handler = new PadTouchZoneHandler(z, heldInputs, aimForwarder, ZONE_TOKEN_BASE + i);
            PadTouchZoneView zv = new PadTouchZoneView(context, z, handler, skin);
            PadTouchZoneHandler.StickSink zoneStick = null;
            if (z.kind == PadTouchZone.Kind.MOVE_STICK) {
                int slotIndex = MOVE_STICK_SLOTS[Math.min(moveSlotIdx++, MOVE_STICK_SLOTS.length - 1)];
                zoneStick = stickSink(slotIndex);
            } else if (z.kind == PadTouchZone.Kind.LOOK_STICK) {
                int slotIndex = LOOK_STICK_SLOTS[Math.min(lookSlotIdx++, LOOK_STICK_SLOTS.length - 1)];
                zoneStick = slotIndex == LOOK_STICK_SLOTS[0] && aimStickSink != null
                        ? aimStickSink : stickSink(slotIndex);
            }
            PadTouchZoneHandler.PointerOut zonePointerOut = z.kind.isLook() ? new PadTouchZoneHandler.PointerOut() {
                @Override
                public void move(int dx, int dy) {
                    if (pointerSink != null) pointerSink.move(dx, dy);
                }

                @Override
                public void tap(int mouseButton) {
                    zv.onTap(mouseButton);
                }
            } : null;
            handler.setSinks(zonePointerOut, zoneStick);
            zv.setPointerSink(pointerSink);
            zoneViews.add(zv);
            frame.addView(zv, zoneInsertAt++, new FrameLayout.LayoutParams(0, 0, Gravity.TOP | Gravity.LEFT));
        }

        int buttonInsertAt = afterLastModernChildIndex();
        for (int i = 0; i < buttons.size(); i++) {
            PadCustomButton b = buttons.get(i);
            PadCustomButtonView bv = new PadCustomButtonView(context, b, skin, heldInputs,
                    aimForwarder, BUTTON_TOKEN_BASE + i);
            bv.setPointerSink(pointerSink);
            bv.setHaptics(hapticsEnabled);
            buttonViews.add(bv);
            frame.addView(bv.haloLayer(), buttonInsertAt++,
                    new FrameLayout.LayoutParams(0, 0, Gravity.TOP | Gravity.LEFT));
        }
        for (PadCustomButtonView bv : buttonViews) {
            frame.addView(bv, buttonInsertAt++, new FrameLayout.LayoutParams(0, 0, Gravity.TOP | Gravity.LEFT));
        }
        place(shown, editing, baseOpacity);
    }

    @SuppressLint("RtlHardcoded")
    void place(boolean shown, boolean editing, float baseOpacity) {
        if (zoneViews.isEmpty() && buttonViews.isEmpty()) return;
        boolean visible = shown && !editing;
        if (!visible) releaseInput();

        int frameW = frame.getWidth();
        int frameH = frame.getHeight();
        int padL = frame.getPaddingLeft();
        int padT = frame.getPaddingTop();
        int padR = frame.getPaddingRight();
        int padB = frame.getPaddingBottom();
        int insetL = explicitSafeInsets ? safeLeft : padL;
        int insetT = explicitSafeInsets ? safeTop : padT;
        int insetR = explicitSafeInsets ? safeRight : padR;
        int insetB = explicitSafeInsets ? safeBottom : padB;
        float density = context.getResources().getDisplayMetrics().density;

        if ((padL | padT | padR | padB) != 0) {
            frame.setClipToPadding(false);
            frame.setClipChildren(false);
        }

        for (PadTouchZoneView zv : zoneViews) {
            zv.setVisibility(visible ? View.VISIBLE : View.GONE);
            zv.setBaseOpacity(baseOpacity);
            if (frameW <= 0 || frameH <= 0) continue;
            zv.zone.pixelBounds(frameW, frameH, box);
            int zLeft = Math.round(box[0]);
            int zTop = Math.round(box[1]);
            int zRight = Math.round(box[0] + box[2]);
            int zBottom = Math.round(box[1] + box[3]);
            FrameLayout.LayoutParams lp = (FrameLayout.LayoutParams) zv.getLayoutParams();
            lp.gravity = Gravity.TOP | Gravity.LEFT;
            lp.width = Math.max(1, zRight - zLeft);
            lp.height = Math.max(1, zBottom - zTop);
            lp.leftMargin = zLeft - padL;
            lp.topMargin = zTop - padT;
            zv.setLayoutParams(lp);
        }

        int safeW = Math.max(0, frameW - insetL - insetR);
        int safeH = Math.max(0, frameH - insetT - insetB);
        for (PadCustomButtonView bv : buttonViews) {
            bv.setVisibility(visible ? View.VISIBLE : View.GONE);
            if (frameW > 0 && frameH > 0) {
                bv.button.bounds(safeW, safeH, density, box);
                FrameLayout.LayoutParams lp = (FrameLayout.LayoutParams) bv.getLayoutParams();
                lp.gravity = Gravity.TOP | Gravity.LEFT;
                lp.width = Math.round(box[2]);
                lp.height = Math.round(box[3]);
                lp.leftMargin = Math.round(box[0]) + (insetL - padL);
                lp.topMargin = Math.round(box[1]) + (insetT - padT);
                bv.setLayoutParams(lp);
                int reach = bv.haloReach();
                FrameLayout.LayoutParams hp = (FrameLayout.LayoutParams) bv.haloLayer().getLayoutParams();
                hp.gravity = Gravity.TOP | Gravity.LEFT;
                hp.width = lp.width + 2 * reach;
                hp.height = lp.height + 2 * reach;
                hp.leftMargin = lp.leftMargin - reach;
                hp.topMargin = lp.topMargin - reach;
                bv.haloLayer().setLayoutParams(hp);
            }
            bv.setRestingAlpha(baseOpacity * bv.button.opacity / 100f);
        }
    }

    void releaseInput() {
        for (PadTouchZoneView zv : zoneViews) zv.releaseInput();
        for (PadCustomButtonView bv : buttonViews) bv.releaseInput();
        aimForwarder.cancelAll();
    }

    void removeViews() {
        releaseInput();
        for (PadTouchZoneView zv : zoneViews) frame.removeView(zv);
        for (PadCustomButtonView bv : buttonViews) {
            frame.removeView(bv);
            frame.removeView(bv.haloLayer());
        }
        zoneViews.clear();
        buttonViews.clear();
    }

    void collectVisibleBounds(List<Rect> out) {
        for (PadCustomButtonView bv : buttonViews) {
            if (bv.button.opacity > 0) OverlayPlacement.addVisible(bv, frame, out);
        }
    }

    List<PadTouchZoneView> zoneViews() {
        return Collections.unmodifiableList(zoneViews);
    }

    List<PadCustomButtonView> buttonViews() {
        return Collections.unmodifiableList(buttonViews);
    }

    private void configureAimFromZones(List<PadTouchZone> zones) {
        for (PadTouchZone z : zones) {
            if (z.kind.isLook()) {
                aimForwarder.reconfigure(z.accelerationCurve, z.sensitivityPercent, z.invertY,
                        z.kind == PadTouchZone.Kind.LOOK_STICK);
                return;
            }
        }
        aimForwarder.reconfigure(null, PadTouchZone.DEFAULT_SENSITIVITY, false, false);
    }

    private PadTouchZoneHandler.StickSink stickSink(int slotIndex) {
        return (right, x, y) -> {
            gamepad.setStick(slotIndex, right ? TouchLayout.RIGHT : TouchLayout.LEFT, x, y);
            gamepadChanged.run();
        };
    }

    private int firstModernChildIndex() {
        int count = frame.getChildCount();
        for (int i = 0; i < count; i++) {
            if (isModernGamepadChild(frame.getChildAt(i))) return i;
        }
        return count;
    }

    private int afterLastModernChildIndex() {
        int last = -1;
        int count = frame.getChildCount();
        for (int i = 0; i < count; i++) {
            if (isModernGamepadChild(frame.getChildAt(i))) last = i;
        }
        return last >= 0 ? last + 1 : count;
    }

    private static boolean isModernGamepadChild(View child) {
        return child instanceof PadReachLayer
                || child instanceof PadControlView.HaloLayer
                || child instanceof PadControlView
                || child instanceof PadControlView.KnobLayer;
    }
}
