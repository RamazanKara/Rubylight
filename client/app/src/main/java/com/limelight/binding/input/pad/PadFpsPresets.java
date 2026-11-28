package com.limelight.binding.input.pad;

import com.limelight.nvstream.input.MouseButtonPacket;

/**
 * Mobile FPS starter presets ({@link Preset#FPS_KBM} and {@link Preset#FPS_GAMEPAD}): left-half
 * floating movement zone, right-half floating look zone, and starter Fire, Scope, Crouch, Reload,
 * Peek Left/Right, and Jump buttons verified non-overlapping and {@code >= 48dp} across all
 * {@code PadLayoutTest.SURFACES} while preserving existing {@link PadLayout.Preset} invariants.
 */
public final class PadFpsPresets {
    public enum Preset { FPS_KBM, FPS_GAMEPAD }

    private PadFpsPresets() {
    }

    /** Applies any preset index from {@code pad_preset_names} (0..2 built-in, 3 FPS KBM, 4 FPS Gamepad). */
    public static void applyPresetChoice(PadLayout layout, int which) {
        if (layout == null) return;
        PadLayout.Preset[] base = PadLayout.Preset.values();
        if (which >= 0 && which < base.length) {
            layout.applyPreset(base[which]);
        } else if (which == base.length) {
            apply(layout, Preset.FPS_KBM);
        } else if (which == base.length + 1) {
            apply(layout, Preset.FPS_GAMEPAD);
        }
    }

    /** Returns true when {@code layout} uses gamepad controls or stick zones rather than WASD/mouse. */
    static boolean isGamepadZoneMode(PadLayout layout) {
        if (layout == null) return false;
        for (PadTouchZone z : layout.touchZones()) {
            if (z.kind == PadTouchZone.Kind.MOVE_STICK || z.kind == PadTouchZone.Kind.LOOK_STICK) {
                return true;
            }
        }
        return layout.spec(PadLayout.Control.WASD).hidden
                && (!layout.spec(PadLayout.Control.FACE).hidden
                || !layout.spec(PadLayout.Control.A).hidden
                || !layout.spec(PadLayout.Control.LEFT_STICK).hidden
                || !layout.spec(PadLayout.Control.RIGHT_STICK).hidden
                || !layout.spec(PadLayout.Control.SELECT).hidden);
    }

    /** Applies an FPS preset onto {@code layout}, preserving Steam Controller control specs. */
    public static void apply(PadLayout layout, Preset preset) {
        if (layout == null || preset == null) return;
        PadLayout.Glyph keptGlyph = layout.glyph();
        PadLayout.PointerMode keptPointerMode = layout.pointerMode();
        boolean keptFacesSwapped = layout.facesSwapped();
        PadLayout.Spec[] keptSteam = new PadLayout.Spec[PadLayout.Control.values().length];
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (!control.steam) continue;
            keptSteam[control.ordinal()] = new PadLayout.Spec(control.hiddenByDefault);
            keptSteam[control.ordinal()].copyFrom(layout.spec(control));
        }

        layout.reset();
        for (PadLayout.Control control : PadLayout.Control.values()) {
            if (control.steam) {
                layout.spec(control).copyFrom(keptSteam[control.ordinal()]);
            } else if (preset == Preset.FPS_GAMEPAD) {
                layout.spec(control).hidden =
                        control != PadLayout.Control.SELECT && control != PadLayout.Control.START;
            } else {
                layout.spec(control).hidden = true;
            }
        }
        layout.setGlyph(keptGlyph);
        layout.setPointerMode(keptPointerMode);
        layout.setFacesSwapped(keptFacesSwapped);

        boolean gamepad = preset == Preset.FPS_GAMEPAD;
        PadTouchZone.applyQuickPreset(layout.mutableTouchZones(), PadTouchZone.QuickPreset.SPLIT_50_50, gamepad);

        PadCustomButton fire = PadCustomButton.mouse("fire", MouseButtonPacket.BUTTON_LEFT, "FIRE", 0.86f, 0.56f);
        fire.setScale(1.15f);
        fire.dragAim = true;

        PadCustomButton scope = PadCustomButton.mouse("scope", MouseButtonPacket.BUTTON_RIGHT, "ADS", 0.88f, 0.30f);

        PadCustomButton jump = PadCustomButton.key("jump", 0x20, "JUMP", 0.88f, 0.82f);
        PadCustomButton reload = PadCustomButton.key("reload", 0x52, "R", 0.66f, 0.42f);
        PadCustomButton crouch = PadCustomButton.key("crouch", 0x43, "C", 0.66f, 0.72f);
        PadCustomButton peekL = PadCustomButton.key("peek_l", 0x51, "Q", 0.11f, 0.28f);
        PadCustomButton peekR = PadCustomButton.key("peek_r", 0x45, "E", 0.29f, 0.28f);

        layout.addCustomButton(fire);
        layout.addCustomButton(scope);
        layout.addCustomButton(crouch);
        layout.addCustomButton(reload);
        layout.addCustomButton(peekL);
        layout.addCustomButton(peekR);
        layout.addCustomButton(jump);
    }
}
