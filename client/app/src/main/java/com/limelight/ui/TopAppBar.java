package com.limelight.ui;

import android.app.Activity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.FrameLayout;
import android.widget.ImageButton;
import android.widget.ScrollView;

import androidx.annotation.DrawableRes;
import androidx.annotation.IdRes;
import androidx.annotation.MenuRes;
import androidx.annotation.StringRes;

import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.R;

/**
 * Configures the shared top app bar (layout/view_top_app_bar.xml) that every screen includes with its
 * own id. Its look comes from the theme's toolbarStyle, so screens only set content and behaviour.
 */
public final class TopAppBar {
    private TopAppBar() {
    }

    /**
     * Sets the bar's title and, when {@code menu} is not 0, inflates its actions. A title also names the window, which a
     * screen reader announces, so a shell screen whose bar is hidden behind its selected tab still says its name.
     */
    public static MaterialToolbar configure(Activity activity, @IdRes int toolbarId, CharSequence title, @MenuRes int menu) {
        MaterialToolbar toolbar = activity.findViewById(toolbarId);
        toolbar.setTitle(title);
        if (title != null && title.length() > 0) activity.setTitle(title);
        if (menu != 0) {
            toolbar.getMenu().clear();
            toolbar.inflateMenu(menu);
        }
        return toolbar;
    }

    /**
     * For layout/view_shell_app_bar.xml. Shown, the shell bar's selected tab names the page: the bar drops its title and
     * is only as tall as the tab row it shares with its actions, so the tabs sit where they do on the screens without a
     * bar; hidden, the title is back at the bar's usual height. The window keeps {@code title} for screen readers.
     */
    public static void showShellBar(Activity activity, MaterialToolbar toolbar, boolean shown, CharSequence title) {
        View bar = toolbar.findViewById(R.id.shell_bar);
        int top = bar.getPaddingTop();
        int below = activity.getResources().getDimensionPixelSize(R.dimen.space_s);
        bar.setVisibility(shown ? View.VISIBLE : View.GONE);
        // The bar's content inset already puts the first tab on the shell inset, where a title would start. The tab
        // row's extra space below moves to the bar, so the actions centre on the tabs rather than on the row.
        bar.setPaddingRelative(0, top, 0, top);
        toolbar.setPaddingRelative(toolbar.getPaddingStart(), toolbar.getPaddingTop(), toolbar.getPaddingEnd(),
                shown ? Math.max(0, below - top) : 0);
        toolbar.setMinimumHeight(shown ? 0 : activity.getResources().getDimensionPixelSize(R.dimen.top_app_bar_height));
        toolbar.setTitle(shown ? null : title);
        activity.setTitle(title);
    }

    /**
     * Adds the leading navigation button and gives it {@code buttonId}, so layouts can name it in
     * nextFocus attributes and tests and the smoke run can find it.
     */
    public static ImageButton setNavigation(MaterialToolbar toolbar, @DrawableRes int icon, @StringRes int description,
                                            @IdRes int buttonId, View.OnClickListener listener) {
        toolbar.setNavigationIcon(icon);
        toolbar.setNavigationContentDescription(description);
        toolbar.setNavigationOnClickListener(listener);
        ImageButton button = navigationButton(toolbar);
        if (button != null && buttonId != View.NO_ID) button.setId(buttonId);
        return button;
    }

    /** Up navigation with the back arrow. */
    public static ImageButton setUpNavigation(MaterialToolbar toolbar, @IdRes int buttonId, View.OnClickListener listener) {
        return setNavigation(toolbar, R.drawable.ic_arrow_back, R.string.settings_navigate_up, buttonId, listener);
    }

    static ImageButton navigationButton(MaterialToolbar toolbar) {
        for (int i = 0; i < toolbar.getChildCount(); i++) {
            View child = toolbar.getChildAt(i);
            if (child instanceof ImageButton && ((ImageButton) child).getDrawable() == toolbar.getNavigationIcon()) {
                return (ImageButton) child;
            }
        }
        return null;
    }

    /**
     * Shows layout/activity_simple_page.xml: a titled bar with up navigation over one content area,
     * which it returns. {@code onNavigateUp} runs for the up button; screens pass their own back
     * handling so, for example, web history still applies.
     */
    public static FrameLayout setUpPage(Activity activity, @StringRes int title, Runnable onNavigateUp) {
        activity.setContentView(R.layout.activity_simple_page);
        activity.setTitle(title);
        MaterialToolbar toolbar = configure(activity, R.id.page_toolbar, activity.getString(title), 0);
        setUpNavigation(toolbar, R.id.page_navigate_up, view -> onNavigateUp.run());
        return activity.findViewById(R.id.page_content);
    }

    /** Puts {@code body} in a scrolling area with the screen gutter and the page's vertical rhythm. */
    public static ScrollView addScrollingBody(FrameLayout page, View body) {
        ScrollView scroll = new ScrollView(page.getContext());
        scroll.setId(R.id.page_scroll);
        scroll.setFillViewport(true);
        scroll.setClipToPadding(false);
        int gutter = page.getResources().getDimensionPixelSize(R.dimen.screen_gutter);
        body.setPaddingRelative(gutter, page.getResources().getDimensionPixelSize(R.dimen.space_s),
                gutter, page.getResources().getDimensionPixelSize(R.dimen.space_xl));
        scroll.addView(body, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT));
        page.addView(scroll, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT));
        return scroll;
    }
}
