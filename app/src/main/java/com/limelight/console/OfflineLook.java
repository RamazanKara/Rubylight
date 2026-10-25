package com.limelight.console;

import android.content.Context;
import android.view.View;
import android.widget.TextView;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;

/**
 * How a card of a PC that is away looks: dimmed, and "PC asleep" on its second line and in what a screen reader says.
 * The card stays focusable and clickable; a game on it opens its page, where it can wake the PC and play.
 */
final class OfflineLook {
    static final float DIMMED = 0.55f;

    private OfflineLook() {
    }

    /** Away only once known to be offline, not while the first poll is still out. */
    static boolean isAway(ComputerDetails host) {
        return host != null && host.state == ComputerDetails.State.OFFLINE;
    }

    /** A card stands for several PCs when the game is on more than one; it is away only when every one of them is. */
    static boolean allAway(java.util.List<String> hostUuids, java.util.function.Function<String, ComputerDetails> hosts) {
        if (hostUuids.isEmpty()) {
            return false;
        }
        for (String uuid : hostUuids) {
            if (!isAway(hosts.apply(uuid))) {
                return false;
            }
        }
        return true;
    }

    /** "On Tower" becomes "On Tower, PC asleep". */
    static String withLabel(Context context, CharSequence line) {
        return context.getString(R.string.console_card_asleep, line);
    }

    /**
     * Applies the look to a bound card, or takes it off a recycled one. Call it after the card's second line, badge
     * and description are set. A card without a second line (a poster on a TV) says it on its badge instead.
     */
    static void bind(View card, TextView subtitle, TextView badge, boolean away) {
        card.setAlpha(away ? DIMMED : 1f);
        if (!away) {
            return;
        }
        Context context = card.getContext();
        if (subtitle != null) {
            subtitle.setText(withLabel(context, subtitle.getText()));
        } else if (badge != null && badge.getVisibility() != View.VISIBLE) {
            badge.setText(R.string.console_badge_asleep);
            badge.setVisibility(View.VISIBLE);
        }
        CharSequence description = card.getContentDescription();
        if (description != null) {
            card.setContentDescription(withLabel(context, description));
        }
    }
}
