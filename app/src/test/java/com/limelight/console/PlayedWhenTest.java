package com.limelight.console;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import android.content.Context;

import org.robolectric.RuntimeEnvironment;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.Calendar;
import java.util.GregorianCalendar;
import java.util.Locale;
import java.util.TimeZone;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, qualifiers = "en-rUS", application = android.app.Application.class)
public class PlayedWhenTest {
    private static final TimeZone BERLIN = TimeZone.getTimeZone("Europe/Berlin");
    private TimeZone zone;
    private Locale locale;
    private Context context;

    @Before
    public void setUp() {
        zone = TimeZone.getDefault();
        locale = Locale.getDefault();
        TimeZone.setDefault(BERLIN);
        Locale.setDefault(Locale.US);
        context = RuntimeEnvironment.getApplication();
    }

    @After
    public void tearDown() {
        TimeZone.setDefault(zone);
        Locale.setDefault(locale);
    }

    private static long at(int month, int day, int hour) {
        Calendar calendar = new GregorianCalendar(BERLIN);
        calendar.clear();
        calendar.set(2026, month, day, hour, 0, 0);
        return calendar.getTimeInMillis();
    }

    private String played(long then, long now) {
        return PlayedWhen.of(context, then, now).toString();
    }

    @Test
    public void lateLastNightIsYesterdayEvenAMinuteOfDayLater() {
        long now = at(Calendar.NOVEMBER, 7, 0);
        assertEquals(1, PlayedWhen.daysBetween(at(Calendar.NOVEMBER, 6, 23), now, BERLIN));
        assertEquals("Yesterday", played(at(Calendar.NOVEMBER, 6, 23), now));
    }

    @Test
    public void earlierTheSameDayIsToday() {
        assertEquals("Today", played(at(Calendar.NOVEMBER, 7, 1), at(Calendar.NOVEMBER, 7, 23)));
    }

    @Test
    public void withinTheWeekIsTheWeekday() {
        // 2026-11-07 is a Saturday.
        assertEquals("Wednesday", played(at(Calendar.NOVEMBER, 4, 12), at(Calendar.NOVEMBER, 7, 12)));
    }

    @Test
    public void theWeekBeforeIsLastWeek() {
        assertEquals("Last week", played(at(Calendar.OCTOBER, 29, 12), at(Calendar.NOVEMBER, 7, 12)));
    }

    @Test
    public void olderKeepsTheDate() {
        String text = played(at(Calendar.AUGUST, 3, 12), at(Calendar.NOVEMBER, 7, 12));
        assertTrue(text, text.contains("2026") && text.contains("3"));
    }

    @Test
    public void aClockChangeDoesNotMoveADay() {
        // Berlin leaves summer time on 2026-10-25: 23 hours short or 25 long, still one day.
        assertEquals(1, PlayedWhen.daysBetween(at(Calendar.OCTOBER, 24, 23), at(Calendar.OCTOBER, 25, 23), BERLIN));
        assertEquals(1, PlayedWhen.daysBetween(at(Calendar.OCTOBER, 25, 0), at(Calendar.OCTOBER, 26, 0), BERLIN));
    }
}
