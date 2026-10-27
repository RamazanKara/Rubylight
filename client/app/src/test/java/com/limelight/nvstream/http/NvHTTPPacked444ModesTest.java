package com.limelight.nvstream.http;

import org.junit.Test;

import static org.junit.Assert.*;

public class NvHTTPPacked444ModesTest {
    private static final String HOST = "<root status_code=\"200\"><PairStatus>1</PairStatus>%s</root>";

    private static ComputerDetails read(String element) throws Exception {
        ComputerDetails details = new ComputerDetails();
        NvHTTP.readClientCapabilities(details, String.format(HOST, element));
        return details;
    }

    @Test
    public void theHostsPackedModesAreRead() throws Exception {
        assertEquals(0b1010, read("<Packed444Modes>10</Packed444Modes>").packed444Modes);
        assertEquals(0, read("<Packed444Modes>0</Packed444Modes>").packed444Modes);
        assertEquals(15, read("<Packed444Modes> 15 </Packed444Modes>").packed444Modes);
        // Modes a newer host adds are left out, the known ones kept.
        assertEquals(0b0101, read("<Packed444Modes>21</Packed444Modes>").packed444Modes);
        assertEquals(0, read("<Packed444Modes>16</Packed444Modes>").packed444Modes);
        // Kept by the copies the computer list hands around.
        ComputerDetails copy = new ComputerDetails(read("<Packed444Modes>5</Packed444Modes>"));
        assertEquals(5, copy.packed444Modes);
    }

    @Test
    public void anAbsentOrUnreadableElementIsUnknown() throws Exception {
        assertEquals(-1, new ComputerDetails().packed444Modes);
        assertEquals(-1, read("").packed444Modes);
        for (String garbage : new String[] {"", "x", "-1", "0x3", "99999999999"}) {
            assertEquals(garbage, -1, read("<Packed444Modes>" + garbage + "</Packed444Modes>").packed444Modes);
        }
        // A refresh that no longer has it forgets the old value.
        ComputerDetails details = read("<Packed444Modes>3</Packed444Modes>");
        details.update(read(""));
        assertEquals(-1, details.packed444Modes);
    }
}
