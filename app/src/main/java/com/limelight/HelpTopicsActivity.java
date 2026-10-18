package com.limelight;

import android.os.Bundle;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.ui.NightjarActivity;

import com.limelight.ui.TopAppBar;
import com.limelight.utils.HelpLauncher;
import com.limelight.utils.ProblemReport;
import com.limelight.utils.UiHelper;

/** Help bundled with the app, so it works offline; each topic links to the web documentation for detail. */
public class HelpTopicsActivity extends NightjarActivity {
    static final String DOCS_URL = "https://github.com/RamazanKara/nightjar/blob/main/docs/";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        UiHelper.setLocale(this);
        super.onCreate(savedInstanceState);
        FrameLayout page = TopAppBar.setUpPage(this, R.string.help, this::finish);
        View topics = getLayoutInflater().inflate(R.layout.help_topics, page, false);
        TopAppBar.addScrollingBody(page, topics);
        // Fire TV apps may not open a browser or a web view, so the topics stand alone there.
        boolean canOpenLinks = !getPackageManager().hasSystemFeature("amazon.hardware.fire_tv");
        link(topics, R.id.help_connect_link, "quick-start.md", canOpenLinks);
        link(topics, R.id.help_pairing_link, "pairing.md", canOpenLinks);
        link(topics, R.id.help_controls_link, "controls.md", canOpenLinks);
        link(topics, R.id.help_profiles_link, "settings.md#named-profiles-and-game-settings", canOpenLinks);
        link(topics, R.id.help_problems_link, "TROUBLESHOOTING.md", canOpenLinks);
        link(topics, R.id.help_report_link, "TROUBLESHOOTING.md#export-a-problem-report", canOpenLinks);
        topics.findViewById(R.id.help_report).setOnClickListener(v -> ProblemReport.show(this));
        UiHelper.notifyNewRootView(this);
    }

    private void link(View topics, int id, String page, boolean canOpenLinks) {
        View link = topics.findViewById(id);
        link.setVisibility(canOpenLinks ? View.VISIBLE : View.GONE);
        // The label lines up with the paragraph above rather than inside the text button's inset.
        link.setPaddingRelative(0, link.getPaddingTop(), link.getPaddingEnd(), link.getPaddingBottom());
        link.setOnClickListener(v -> HelpLauncher.launchUrl(this, DOCS_URL + page));
    }
}
