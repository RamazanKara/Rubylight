/** Which "HDR format" values a screen is offered, and how stored values read back. */
package com.limelight.preferences;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

import java.util.Arrays;

public class HdrFormatChoiceTest {
    @Test
    public void aScreenWithoutDolbyVisionLeavesOutNoDolbyVisionUnlessItIsTheStoredChoice() {
        assertEquals(Arrays.asList(HdrFormatChoice.AUTO, HdrFormatChoice.HDR10),
                HdrFormatChoice.offered(false, HdrFormatChoice.AUTO));
        // A choice stored on a Dolby Vision display stays listed, so opening settings keeps it.
        assertEquals(Arrays.asList(HdrFormatChoice.values()),
                HdrFormatChoice.offered(false, HdrFormatChoice.NO_DOLBY_VISION));
        assertEquals(Arrays.asList(HdrFormatChoice.values()),
                HdrFormatChoice.offered(true, HdrFormatChoice.HDR10));
    }

    @Test
    public void storedValuesReadBackAndUnknownOnesAreAutomatic() {
        for (HdrFormatChoice choice : HdrFormatChoice.values()) {
            assertEquals(choice, HdrFormatChoice.fromPreference(choice.value));
        }
        assertEquals(HdrFormatChoice.AUTO, HdrFormatChoice.fromPreference("dolby_vision"));
        assertEquals(HdrFormatChoice.AUTO, HdrFormatChoice.fromPreference(null));
    }
}
