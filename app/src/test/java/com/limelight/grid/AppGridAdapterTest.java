package com.limelight.grid;

import com.limelight.R;
import com.limelight.nvstream.http.NvApp;
import org.junit.Test;
import static org.junit.Assert.*;

public class AppGridAdapterTest {
    @Test
    public void remoteTilesDescribeTheirRole() {
        NvApp monitor = new NvApp("Monitor", 2147483505, false);
        NvApp input = new NvApp("Input", 2147483506, false);
        assertEquals(R.string.role_view_only, AppGridAdapter.roleLabel(monitor));
        assertEquals(R.string.role_input_only, AppGridAdapter.roleLabel(input));
        assertEquals(0, AppGridAdapter.roleLabel(new NvApp("Game", 10, false)));
    }

    @Test
    public void endRoleTilesDescribeTheActionRatherThanAStream() {
        assertEquals(R.string.stream_end_monitor,
                AppGridAdapter.roleLabel(new NvApp("End monitor", 2147483502, false)));
        assertEquals(R.string.stream_end_input,
                AppGridAdapter.roleLabel(new NvApp("End input", 2147483503, false)));
    }
}
