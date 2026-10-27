package com.limelight.ui;

import android.content.Context;

import androidx.annotation.StringRes;

import com.limelight.R;
import com.limelight.nvstream.http.HostCapabilities;

/**
 * How a control for a host feature appears on a PC's menu and per-PC screens:
 * <ul>
 * <li>shown and enabled when the host supports the feature,</li>
 * <li>shown and enabled with "Applies if the host supports it" while the host is unknown,</li>
 * <li>shown disabled with a short reason when a host setting, permission, pairing or update
 * would make it available,</li>
 * <li>hidden when the host can never offer it.</li>
 * </ul>
 * Global settings have no host and show everything; in-stream menus show only what
 * {@link HostCapabilities#has} confirms.
 */
public final class HostFeatureGate {
    public enum Presentation { ENABLED, DISABLED, HIDDEN }

    private static final HostFeatureGate SUPPORTED = new HostFeatureGate(Presentation.ENABLED, 0);
    private static final HostFeatureGate UNKNOWN = new HostFeatureGate(Presentation.ENABLED, R.string.host_feature_unknown);
    private static final HostFeatureGate HIDDEN = new HostFeatureGate(Presentation.HIDDEN, 0);

    public final Presentation presentation;
    /** Text shown with the control: the reason it is disabled, or the unknown-host note; 0 for none. */
    @StringRes public final int note;

    private HostFeatureGate(Presentation presentation, @StringRes int note) {
        this.presentation = presentation;
        this.note = note;
    }

    public static HostFeatureGate of(HostCapabilities caps, HostCapabilities.Feature feature) {
        HostCapabilities.Availability availability = caps.availability(feature);
        switch (availability.state) {
            case SUPPORTED:
                return SUPPORTED;
            case UNKNOWN:
                return UNKNOWN;
            default:
                return availability.isNever() ? HIDDEN :
                        new HostFeatureGate(Presentation.DISABLED, availability.reasonText());
        }
    }

    public boolean isVisible() {
        return presentation != Presentation.HIDDEN;
    }

    public boolean isEnabled() {
        return presentation == Presentation.ENABLED;
    }

    /** {@code text}, followed on a new line by the note when there is one. */
    public CharSequence withNote(Context context, CharSequence text) {
        if (note == 0) {
            return text;
        }
        return text == null || text.length() == 0 ? context.getString(note) : text + "\n" + context.getString(note);
    }
}
