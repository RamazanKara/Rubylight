package com.limelight.preferences;

import com.limelight.R;

/**
 * Named starting points for one game's stream settings. A preset only fills in values the per-game
 * layer already has (frame pacing, adaptive refresh and bitrate); nothing new is stored, so a game
 * shows a preset when those three values match it and Custom otherwise. Everything else, such as
 * resolution, refresh rate and codec, stays as the PC or the main settings have it.
 *
 * <ul>
 * <li>Competitive: lowest-latency frame pacing, the app default, set for this game so it holds when
 * the main setting is changed. Bitrate follows the PC.</li>
 * <li>Smoothness: the smoothest frame pacing, which holds frames back and adds delay, with adaptive
 * refresh off for the game because it replaces frame pacing.</li>
 * <li>Quality: 50% more bitrate than this game would otherwise use, up to the host limit. Frame
 * pacing is unchanged. On a weak link the larger frames can add delay.</li>
 * <li>Custom: any other combination.</li>
 * </ul>
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

    /** The bitrate Quality sets, given what the game would use without its own value. */
    static int qualityBitrate(int baseBitrateKbps) {
        long wanted = (long) baseBitrateKbps * QUALITY_BITRATE_PERCENT / 100;
        return (int) Math.min(HostStreamProfile.MAX_BITRATE_KBPS, wanted);
    }

    /**
     * Sets the values of this preset on {@code own}; Custom changes nothing.
     *
     * @param baseBitrateKbps the bitrate the game uses when it has none of its own
     */
    void apply(HostStreamProfile own, int baseBitrateKbps) {
        switch (this) {
            case COMPETITIVE:
                own.pacing = PreferenceConfiguration.DEFAULT_FRAME_PACING;
                own.vrr = null;
                own.bitrate = null;
                break;
            case SMOOTHNESS:
                own.pacing = "smoothness";
                own.vrr = Boolean.FALSE;
                own.bitrate = null;
                break;
            case QUALITY:
                own.pacing = null;
                own.vrr = null;
                own.bitrate = qualityBitrate(baseBitrateKbps);
                break;
            default:
                break;
        }
    }

    /** The preset whose values {@code own} holds, or Custom. */
    static GameProfilePreset of(HostStreamProfile own, int baseBitrateKbps) {
        if (own.vrr == null && own.bitrate == null &&
                PreferenceConfiguration.DEFAULT_FRAME_PACING.equals(own.pacing)) {
            return COMPETITIVE;
        }
        if (Boolean.FALSE.equals(own.vrr) && own.bitrate == null && "smoothness".equals(own.pacing)) {
            return SMOOTHNESS;
        }
        if (own.pacing == null && own.vrr == null && own.bitrate != null &&
                own.bitrate == qualityBitrate(baseBitrateKbps)) {
            return QUALITY;
        }
        return CUSTOM;
    }
}
