package com.limelight.binding.input.keyboard;

import java.util.ArrayList;
import java.util.EnumMap;
import java.util.List;

/**
 * Turns presses on the on-screen keyboard into key events for the host, and guarantees that every
 * key and modifier it pressed is released again. It has no view or connection: the owner passes the
 * time in and receives each event through {@link Output}.
 *
 * <p>A key is sent down when it is pressed and up when it is released, so a game sees it held. While it
 * is held it repeats like a PC keyboard. A modifier that was only armed is held down for the one key
 * that uses it; a locked modifier is held from the moment it locks until it unlocks.
 */
public final class PcKeyboardEngine {
    /** The first repeat comes after this long, then one every {@link #REPEAT_MS}. */
    public static final long REPEAT_DELAY_MS = 400;
    public static final long REPEAT_MS = 40;

    public interface Output {
        /** One key event for the host; {@code modifiers} are the {@code KeyboardPacket.MODIFIER_*} bits then down. */
        void send(int vk, boolean down, byte modifiers);
    }

    private static final class Held {
        final Object owner;
        final PcKey key;
        final List<PcModifier> usedUp = new ArrayList<>(2);
        long nextRepeat;

        Held(Object owner, PcKey key) {
            this.owner = owner;
            this.key = key;
        }
    }

    private static final PcModifier[] MODIFIERS = PcModifier.values();
    /** A key Windows ignores, sent between a lone Alt or Win going down and up so it is not taken as a tap. */
    public static final int NEUTRAL_VK = 0xE8;

    private final Output output;
    private final StickyModifiers sticky = new StickyModifiers();
    private final EnumMap<PcModifier, Boolean> hostDown = new EnumMap<>(PcModifier.class);
    /** Whether a key was pressed while the modifier was down, so letting it go is not a lone tap. */
    private final boolean[] combined = new boolean[MODIFIERS.length];
    private final List<Held> held = new ArrayList<>();

    public PcKeyboardEngine(Output output) {
        this.output = output;
    }

    public StickyModifiers modifiers() {
        return sticky;
    }

    /** Whether any key is held, so the owner knows to keep calling {@link #tick}. */
    public boolean hasHeldKeys() {
        return !held.isEmpty();
    }

    public boolean isHeld(Object owner) {
        return find(owner) != null;
    }

    /** Whether the key is down now, for any owner; the view draws it pressed. */
    public boolean isKeyHeld(PcKey key) {
        for (int i = 0; i < held.size(); i++) {
            if (held.get(i).key == key) return true;
        }
        return false;
    }

    /** Press a key for the given owner (a touch pointer or a controller button). A modifier key only changes state. */
    public void press(PcKey key, Object owner, long now) {
        if (key.modifier != null) {
            tapModifier(key.modifier);
            return;
        }
        if (find(owner) != null) return;
        Held entry = new Held(owner, key);
        for (PcModifier modifier : MODIFIERS) {
            if (!sticky.isActive(modifier)) continue;
            setHostDown(modifier, true);
            combined[modifier.ordinal()] = true;
            // The key uses an armed modifier up; a locked one stays down for the keys after it.
            if (sticky.state(modifier) == StickyModifiers.State.ONCE) entry.usedUp.add(modifier);
        }
        entry.nextRepeat = now + REPEAT_DELAY_MS;
        held.add(entry);
        output.send(key.vk, true, hostBits());
        for (PcModifier modifier : entry.usedUp) sticky.set(modifier, StickyModifiers.State.OFF);
    }

    /** Release the key the owner pressed, and the armed modifiers it used. */
    public void release(Object owner) {
        Held entry = find(owner);
        if (entry == null) return;
        held.remove(entry);
        output.send(entry.key.vk, false, hostBits());
        for (PcModifier modifier : entry.usedUp) {
            if (sticky.state(modifier) != StickyModifiers.State.LOCKED && !usedByHeldKey(modifier)) {
                setHostDown(modifier, false);
            }
        }
    }

    /** Repeat held keys that are due. Allocation-free. */
    public void tick(long now) {
        for (int i = 0; i < held.size(); i++) {
            Held entry = held.get(i);
            if (!entry.key.repeats || now < entry.nextRepeat) continue;
            entry.nextRepeat = now + REPEAT_MS;
            output.send(entry.key.vk, true, hostBits());
        }
    }

    /** Tap a modifier: arm it, lock it, or release it. */
    public void tapModifier(PcModifier modifier) {
        StickyModifiers.State state = sticky.tap(modifier);
        // Armed modifiers reach the host with the next key; a locked one is held from now on. One a held
        // key still uses is let go with that key.
        if (state == StickyModifiers.State.LOCKED) setHostDown(modifier, true);
        else if (state == StickyModifiers.State.OFF && !usedByHeldKey(modifier)) setHostDown(modifier, false);
    }

    /** Release every key and modifier this engine pressed and forget the sticky state. */
    public void releaseAll() {
        for (int i = held.size() - 1; i >= 0; i--) {
            Held entry = held.remove(i);
            output.send(entry.key.vk, false, hostBits());
        }
        for (PcModifier modifier : MODIFIERS) setHostDown(modifier, false);
        sticky.clear();
    }

    private Held find(Object owner) {
        for (int i = 0; i < held.size(); i++) {
            if (held.get(i).owner.equals(owner)) return held.get(i);
        }
        return null;
    }

    private boolean usedByHeldKey(PcModifier modifier) {
        for (int i = 0; i < held.size(); i++) {
            if (held.get(i).usedUp.contains(modifier)) return true;
        }
        return false;
    }

    private boolean isHostDown(PcModifier modifier) {
        return Boolean.TRUE.equals(hostDown.get(modifier));
    }

    private void setHostDown(PcModifier modifier, boolean down) {
        if (isHostDown(modifier) == down) return;
        if (down) {
            combined[modifier.ordinal()] = false;
        } else if (!combined[modifier.ordinal()] && (modifier == PcModifier.ALT || modifier == PcModifier.WIN)) {
            // Alt or Win let go with nothing between would open the menu bar or the Start menu, as on a PC.
            output.send(NEUTRAL_VK, true, hostBits());
            output.send(NEUTRAL_VK, false, hostBits());
        }
        hostDown.put(modifier, down);
        output.send(modifier.vk, down, hostBits());
    }

    private byte hostBits() {
        int bits = 0;
        for (PcModifier modifier : MODIFIERS) {
            if (isHostDown(modifier)) bits |= modifier.bit;
        }
        return (byte) bits;
    }
}
