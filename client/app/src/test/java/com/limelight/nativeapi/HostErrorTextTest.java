package com.limelight.nativeapi;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.fail;

import android.app.Application;
import android.content.res.Resources;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

/** The host's title, next step and technical detail reach the error text, and their absence changes nothing. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostErrorTextTest {
    private static final String NEXT = "What to do: %1$s";
    private static final String DETAIL = "Technical details: %1$s";

    private static NativeApiException decoded(String error) {
        try {
            NativeApiClient.decode("{\"error\":" + error + "}");
            fail("an error envelope throws");
            return null;
        } catch (NativeApiException e) {
            return e;
        }
    }

    private static Resources resources() {
        return RuntimeEnvironment.getApplication().getResources();
    }

    @Test
    public void composeKeepsTheLeadAloneWithoutTheParts() {
        assertEquals("TV is streaming from this PC.",
                HostErrorText.compose(null, "TV is streaming from this PC.", null, null, NEXT, DETAIL));
        assertEquals("words", HostErrorText.compose(" ", "words", "", null, NEXT, DETAIL));
        assertNull(HostErrorText.compose(null, null, null, null, NEXT, DETAIL));
    }

    @Test
    public void composePutsEachPartInItsOwnParagraph() {
        assertEquals("The PC is in use\n\nTV is streaming from this PC.\n\nWhat to do: Wait.\n\nTechnical details: 0x1",
                HostErrorText.compose("The PC is in use", "TV is streaming from this PC.", "Wait.", "0x1", NEXT, DETAIL));
        assertEquals("Same words\n\nWhat to do: Wait.",
                HostErrorText.compose("Same words", "Same words", "Wait.", null, NEXT, DETAIL));
        assertEquals("Too many wrong PINs",
                HostErrorText.compose("Too many wrong PINs", "", null, null, NEXT, DETAIL));
    }

    @Test
    public void aNewHostsErrorCarriesItsPartsIntoTheDialogText() {
        NativeApiException e = decoded("{\"kind\":\"api\",\"message\":\"wrote 3 and the driver reads back 1\","
                + "\"http_status\":409,\"code\":\"driver_rejected\",\"setting\":\"anti_lag.level\","
                + "\"title\":\"The graphics driver refused the change\",\"next_step\":\"Choose another value.\","
                + "\"technical_detail\":\"driver_rejected (anti_lag.level): ADLX_FAIL\"}");
        assertEquals("The graphics driver refused the change", e.getTitle());
        assertEquals("Choose another value.", e.getNextStep());
        assertEquals("driver_rejected (anti_lag.level): ADLX_FAIL", e.getTechnicalDetail());
        assertEquals("The graphics driver refused the change\n\nwrote 3 and the driver reads back 1\n\n"
                        + "What to do: Choose another value.\n\n"
                        + "Technical details: driver_rejected (anti_lag.level): ADLX_FAIL",
                HostErrorText.of(resources(), e));
        assertEquals("Nightjar's words\n\nWhat to do: Choose another value.\n\n"
                        + "Technical details: driver_rejected (anti_lag.level): ADLX_FAIL",
                HostErrorText.withHelp(resources(), "Nightjar's words", e.getNextStep(), e.getTechnicalDetail()));
    }

    @Test
    public void anOlderHostsErrorReadsAsBefore() {
        NativeApiException e = decoded("{\"kind\":\"api\",\"message\":\"That is not the PIN TV shows.\","
                + "\"http_status\":422,\"code\":\"wrong_pin\",\"setting\":null,\"retry_after_ms\":null}");
        assertNull(e.getTitle());
        assertNull(e.getNextStep());
        assertNull(e.getTechnicalDetail());
        assertEquals("That is not the PIN TV shows.", HostErrorText.of(resources(), e));
        assertEquals("Couldn't: That is not the PIN TV shows.",
                HostErrorText.around(resources(), "Couldn't: That is not the PIN TV shows.", e));
        assertEquals("Nightjar's words", HostErrorText.withHelp(resources(), "Nightjar's words", null, null));
    }
}
