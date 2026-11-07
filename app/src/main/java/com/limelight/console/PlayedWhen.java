package com.limelight.console;

import android.content.Context;
import android.icu.text.DisplayContext;
import android.icu.text.RelativeDateTimeFormatter;
import android.icu.util.ULocale;
import android.os.Build;
import android.text.format.DateUtils;

import androidx.core.os.ConfigurationCompat;

import java.util.Calendar;
import java.util.Date;
import java.util.Locale;
import java.util.TimeZone;

/**
 * When a game was last played, the way a person says it: Today, Yesterday, the weekday, Last week; a date once it is older.
 * The words come from the platform in the device's language, so they need no translations of ours.
 */
final class PlayedWhen {
    private PlayedWhen() {
    }

    /** Calendar days from {@code thenMs} to {@code nowMs} in this zone; a clock change does not move a day. */
    static int daysBetween(long thenMs, long nowMs, TimeZone zone) {
        return (int) (dayNumber(nowMs, zone) - dayNumber(thenMs, zone));
    }

    private static long dayNumber(long ms, TimeZone zone) {
        Calendar day = Calendar.getInstance(zone);
        day.setTimeInMillis(ms);
        // A date in UTC at noon: whole days apart whatever the zone's offset did in between.
        Calendar utc = Calendar.getInstance(TimeZone.getTimeZone("UTC"));
        utc.clear();
        utc.set(day.get(Calendar.YEAR), day.get(Calendar.MONTH), day.get(Calendar.DAY_OF_MONTH), 12, 0, 0);
        return utc.getTimeInMillis() / DateUtils.DAY_IN_MILLIS;
    }

    static CharSequence of(Context context, long thenMs, long nowMs) {
        Locale locale = ConfigurationCompat.getLocales(context.getResources().getConfiguration()).get(0);
        int days = daysBetween(thenMs, nowMs, TimeZone.getDefault());
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && days >= 0 && days < 14) {
            if (days >= 2 && days < 7) return sentence(new android.icu.text.SimpleDateFormat("cccc", locale).format(new Date(thenMs)), locale);
            RelativeDateTimeFormatter words = RelativeDateTimeFormatter.getInstance(ULocale.forLocale(locale), null,
                    RelativeDateTimeFormatter.Style.LONG, DisplayContext.CAPITALIZATION_FOR_BEGINNING_OF_SENTENCE);
            if (days == 0) {
                return words.format(RelativeDateTimeFormatter.Direction.THIS, RelativeDateTimeFormatter.AbsoluteUnit.DAY);
            }
            if (days == 1) {
                return words.format(RelativeDateTimeFormatter.Direction.LAST, RelativeDateTimeFormatter.AbsoluteUnit.DAY);
            }
            return words.format(RelativeDateTimeFormatter.Direction.LAST, RelativeDateTimeFormatter.AbsoluteUnit.WEEK);
        }
        return DateUtils.formatDateTime(context, thenMs, DateUtils.FORMAT_SHOW_DATE | DateUtils.FORMAT_SHOW_YEAR);
    }

    private static String sentence(String word, Locale locale) {
        if (word.isEmpty()) return word;
        return word.substring(0, 1).toUpperCase(locale) + word.substring(1);
    }
}
