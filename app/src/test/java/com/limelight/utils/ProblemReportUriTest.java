package com.limelight.utils;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.res.XmlResourceParser;

import com.limelight.BuildConfig;
import com.limelight.R;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.xmlpull.v1.XmlPullParser;

/** The report's URI, which Send to PC compares with, is the one the reports provider serves for the report. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class ProblemReportUriTest {
    @Test
    public void theReportUriNamesTheProvidersCacheRootForTheReportsFolder() throws Exception {
        assertEquals("content://" + BuildConfig.APPLICATION_ID + ".reports/reports/nightjar-problem.txt",
                ProblemReport.reportUri().toString());
        // FileProvider gives <authority>/<root name>/<path under the root>: the root named "reports" must be the
        // cache folder the report is written to, or Send to PC would refuse the report it was opened with.
        boolean found = false;
        try (XmlResourceParser paths = RuntimeEnvironment.getApplication().getResources().getXml(R.xml.report_paths)) {
            for (int event = paths.getEventType(); event != XmlPullParser.END_DOCUMENT; event = paths.next()) {
                if (event == XmlPullParser.START_TAG
                        && ProblemReport.REPORT_ROOT.equals(paths.getAttributeValue(null, "name"))) {
                    assertEquals("cache-path", paths.getName());
                    assertEquals(ProblemReport.REPORT_ROOT + "/",
                            paths.getAttributeValue(null, "path"));
                    found = true;
                }
            }
        }
        assertTrue(found);
    }
}
