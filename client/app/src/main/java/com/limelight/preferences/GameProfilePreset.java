package com.limelight.preferences;

import com.limelight.R;

/**
 * Named starting points for one game's stream settings. Each preset sets only the values it names
 * and leaves every other value of the game alone; nothing new is stored, so a game shows a preset
 * when the values that preset names match it. Resolution, refresh rate, codec and the like stay as
 * the PC or the main settings have them.
 *
 * <ul>
 * <li>Competitive sets frame pacing to lowest latency, the app default, so it holds when the main
 * setting is changed. Nothing else.</li>
 * <li>Smoothness sets the smoothest frame pacing, which holds frames back and adds delay, and turns
 * adaptive refresh off for the game because it replaces frame pacing. Bitrate is not touched.</li>
 * <li>Quality sets the bitrate to 150% of what the PC or the main settings use, up to the highest
 * bitrate the host accepts. Frame pacing and adaptive refresh are not touched. On a weak link the
 * larger frames can add delay.</li>
 * <li>Custom: any other combination, including two presets' values together.</li>
 * </ul>
 *
 * The Quality base is the PC's bitrate if it has one, else the main settings'. A network profile is
 * left out on purpose: which one applies changes with the network, and the preset must not.
 *
 * Radeon settings per game (for example Anti-Lag or Chill) are not part of the native API yet, so no
 * preset changes them; that part waits for host support.
 */
public enum GameProfilePreset {
    COMPETITIVE(R.string.game_preset_competitive, R.string.game_preset_competitive_summary),
    SMOOTHNESS(R.string.game_preset_smoothness, R.string.game_preset_smoothness_summary),
    QUALITY(R.string.game_preset_quality, R.string.game_preset_quality_summary),
    CUSTOM(R.string.game_preset_custom, R.string.game_preset_custom_summary);

    /** Quality asks for this share of the bitrate the game would otherwise use. */
    static final int QUALITY_BITRATE_PERCENT = 150;

    final int title;
    final int summary;

    GameProfilePreset(int title, int summary) {
        this.title = title;
        this.summary = summary;
    }

    /**
     * The bitrate the game would use without a value of its own, leaving out the network profile: the
     * PC's value when it has one, else the main settings'.
     */
    static int baseBitrate(int mainKbps, HostStreamProfile pc) {
        return pc != null && pc.bitrate != null ? pc.bitrate : mainKbps;
    }

    /**
     * The bitrate Quality sets: 150% of {@code baseBitrateKbps}, at most what the host accepts and
     * the bitrate slider offers ({@link HostStreamProfile#MAX_BITRATE_KBPS}).
     */
    static int qualityBitrate(int baseBitrateKbps) {
        long wanted = (long) baseBitrateKbps * QUALITY_BITRATE_PERCENT / 100;
        return (int) Math.max(500, Math.min(HostStreamProfile.MAX_BITRATE_KBPS, wanted));
    }

    /**
     * Sets the values this preset names on {@code own} and no others; Custom changes nothing.
     *
     * @param qualityKbps what Quality sets, from {@link #qualityBitrate}
     */
    void apply(HostStreamProfile own, int qualityKbps) {
        switch (this) {
            case COMPETITIVE:
                own.pacing = PreferenceConfiguration.DEFAULT_FRAME_PACING;
                break;
            case SMOOTHNESS:
                own.pacing = "smoothness";
                own.vrr = Boolean.FALSE;
                break;
            case QUALITY:
                own.bitrate = qualityKbps;
                break;
            default:
                break;
        }
    }

    /** The one preset whose values {@code own} holds, or Custom when none or several match. */
    static GameProfilePreset of(HostStreamProfile own, int qualityKbps) {
        boolean competitive = PreferenceConfiguration.DEFAULT_FRAME_PACING.equals(own.pacing);
        boolean smoothness = "smoothness".equals(own.pacing) && Boolean.FALSE.equals(own.vrr);
        boolean quality = own.bitrate != null && own.bitrate == qualityKbps;
        int matches = (competitive ? 1 : 0) + (smoothness ? 1 : 0) + (quality ? 1 : 0);
        if (matches != 1) {
            return CUSTOM;
        }
        return competitive ? COMPETITIVE : smoothness ? SMOOTHNESS : QUALITY;
    }
}
