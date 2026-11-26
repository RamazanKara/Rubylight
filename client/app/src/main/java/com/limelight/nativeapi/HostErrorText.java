package com.limelight.nativeapi;

import android.content.res.Resources;

import com.limelight.R;

/**
 * The PC's plain-English parts of a native API error (title, next step, technical detail) around the
 * words an error dialog already shows. A host that sends none of them (an older Rubylight, Sunshine,
 * Apollo, GameStream) leaves the dialog's words exactly as they were: the parts are the gate, never the
 * host's brand.
 */
public final class HostErrorText {
    private HostErrorText() {
    }

    /**
     * {@code lead} with {@code title} above it and {@code nextStep} and {@code technicalDetail} below, each its
     * own paragraph; {@code lead} alone when the others are null or blank. The formats take one {@code %1$s}.
     */
    public static String compose(String title, String lead, String nextStep, String technicalDetail,
                                 String nextStepFormat, String technicalDetailFormat) {
        if (!given(title) && !given(nextStep) && !given(technicalDetail)) {
            return lead;
        }
        StringBuilder text = new StringBuilder();
        String shownLead = lead == null ? "" : lead;
        if (given(title) && !title.trim().equals(shownLead.trim())) {
            append(text, title.trim());
        }
        if (!shownLead.isEmpty()) {
            append(text, shownLead);
        }
        if (given(nextStep)) {
            append(text, String.format(nextStepFormat, nextStep.trim()));
        }
        if (given(technicalDetail)) {
            append(text, String.format(technicalDetailFormat, technicalDetail.trim()));
        }
        return text.toString();
    }

    /** The host's own message with all its parts: for a dialog that shows the PC's words as they came. */
    public static String of(Resources resources, NativeApiException error) {
        return around(resources, error.getMessage(), error);
    }

    /** {@code lead}, which already carries the host's message, with the host's title, next step and detail. */
    public static String around(Resources resources, String lead, NativeApiException error) {
        if (!given(error.getTitle()) && !given(error.getNextStep()) && !given(error.getTechnicalDetail())) {
            return lead;
        }
        return compose(error.getTitle(), lead, error.getNextStep(), error.getTechnicalDetail(),
                resources.getString(R.string.host_error_next_step),
                resources.getString(R.string.host_error_technical_detail));
    }

    /** Nightjar's own words for an error with the host's next step and detail; its title would repeat them. */
    public static String withHelp(Resources resources, String lead, String nextStep, String technicalDetail) {
        if (!given(nextStep) && !given(technicalDetail)) {
            return lead;
        }
        return compose(null, lead, nextStep, technicalDetail,
                resources.getString(R.string.host_error_next_step),
                resources.getString(R.string.host_error_technical_detail));
    }

    private static boolean given(String part) {
        return part != null && !part.trim().isEmpty();
    }

    private static void append(StringBuilder text, String part) {
        if (text.length() > 0) {
            text.append("\n\n");
        }
        text.append(part);
    }
}
