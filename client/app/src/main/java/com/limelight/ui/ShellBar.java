package com.limelight.ui;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.view.LayoutInflater;
import android.view.Menu;
import android.view.MenuItem;
import android.view.View;
import android.view.ViewGroup;
import android.widget.LinearLayout;
import android.widget.PopupMenu;

import androidx.activity.ComponentActivity;
import androidx.activity.OnBackPressedCallback;
import androidx.annotation.IdRes;
import androidx.core.view.ViewCompat;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.console.ConsoleHomeActivity;
import com.limelight.console.HomeSettings;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.preferences.StreamSettings;

/**
 * The shell bar: Home, PCs and Settings under the top app bar of a top-level screen (layout/view_shell_bar.xml,
 * buttons from menu/shell.xml). It also holds the navigation rules the three destinations share:
 * <ul>
 * <li>The root is Home when the app opens on Home ({@link HomeSettings#opensOnHome}), otherwise PCs.
 * <li>Back from a destination that is not the root goes to the root; Back on the root leaves the app.
 * <li>Switching destinations replaces the current one instead of stacking: the root is brought back with
 *     CLEAR_TOP and SINGLE_TOP, and a screen that is not the root finishes when it opens a sibling.
 * <li>Home is hidden while no host has a library to show, except on Home's own screens.
 * </ul>
 */
public final class ShellBar {
    public enum Destination {
        HOME(R.id.shell_home), PCS(R.id.shell_pcs), SETTINGS(R.id.shell_settings);

        @IdRes public final int buttonId;

        Destination(@IdRes int buttonId) {
            this.buttonId = buttonId;
        }
    }

    /**
     * Set on the intent that opens Settings from the bar. Settings applies the shell Back rule only then;
     * opened any other way (Ctrl+comma, a menu), Back simply returns to the screen it came from.
     */
    public static final String EXTRA_FROM_SHELL = "com.limelight.ui.FROM_SHELL";

    private ShellBar() {
    }

    /** Where Back from any other destination ends: Home when the app opens on it, else the list of PCs. */
    public static Destination root(Context context) {
        return HomeSettings.opensOnHome(context) ? Destination.HOME : Destination.PCS;
    }

    /** Home appears in the bar once some host has a library; until then it would be an empty screen. */
    public static boolean homeAvailable(Context context) {
        return ConsoleRepository.hasAnyLibrary(context);
    }

    /** The intent that opens a destination, without the task flags {@link #open} adds. */
    public static Intent intentFor(Context context, Destination destination) {
        switch (destination) {
            case HOME:
                return new Intent(context, ConsoleHomeActivity.class);
            case SETTINGS:
                return new Intent(context, StreamSettings.class).putExtra(EXTRA_FROM_SHELL, true);
            default:
                // Stays on the list instead of forwarding to Home the way a launcher start does.
                return HomeSettings.pcList(context);
        }
    }

    /**
     * Fills {@code R.id.shell_bar} (from layout/view_shell_bar.xml) and marks {@code current}. Each button
     * keeps the id of its menu item, so tests and the smoke run can find it.
     */
    public static void install(Activity activity, Destination current) {
        View bar = activity.findViewById(R.id.shell_bar);
        LinearLayout items = bar.findViewById(R.id.shell_bar_items);
        items.removeAllViews();
        ViewCompat.setAccessibilityPaneTitle(bar, activity.getString(R.string.shell_destinations));

        Menu menu = new PopupMenu(activity, bar).getMenu();
        activity.getMenuInflater().inflate(R.menu.shell, menu);
        LayoutInflater inflater = LayoutInflater.from(activity);
        int gap = activity.getResources().getDimensionPixelSize(R.dimen.space_s);
        for (int i = 0; i < menu.size(); i++) {
            MenuItem item = menu.getItem(i);
            MaterialButton button = (MaterialButton) inflater.inflate(R.layout.view_shell_item, items, false);
            button.setId(item.getItemId());
            button.setText(item.getTitle());
            button.setIcon(item.getIcon());
            Destination destination = destinationOf(item.getItemId());
            boolean selected = destination == current;
            button.setSelected(selected);
            if (selected) {
                ViewCompat.setStateDescription(button, activity.getString(R.string.shell_current_page));
            }
            button.setOnClickListener(v -> open(activity, current, destination));
            ViewGroup.MarginLayoutParams params = (ViewGroup.MarginLayoutParams) button.getLayoutParams();
            params.setMarginEnd(gap);
            items.addView(button, params);
        }
        refresh(activity);
    }

    /**
     * Shows or hides Home; screens call it when they come back, since a library can appear while they are away.
     * On Home's own screens the tab stays: Home opens before its first library is saved (a new PC, cleared data), and
     * a hidden current tab left the bar without it and sent Up from the shelves to PCs.
     */
    public static void refresh(Activity activity) {
        View home = activity.findViewById(R.id.shell_home);
        if (home != null) {
            home.setVisibility(home.isSelected() || homeAvailable(activity) ? View.VISIBLE : View.GONE);
        }
    }

    private static Destination destinationOf(@IdRes int id) {
        for (Destination destination : Destination.values()) {
            if (destination.buttonId == id) return destination;
        }
        throw new IllegalArgumentException("Not a destination: " + id);
    }

    /** Opens {@code target} from {@code current}, replacing the current screen unless it is the root. */
    public static void open(Activity activity, Destination current, Destination target) {
        if (target == current) return;
        Destination root = root(activity);
        Intent intent = intentFor(activity, target);
        if (target == root) {
            intent.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        }
        activity.startActivity(intent);
        // The root stays under whatever it opens; any other destination makes way for the next.
        if (current != root) {
            activity.finish();
        }
    }

    /** Back from {@code current}: to the root, or out of the app when {@code current} is the root. */
    public static void leave(Activity activity, Destination current) {
        if (current == root(activity)) {
            activity.finish();
        } else {
            open(activity, current, root(activity));
        }
    }

    /**
     * Makes system Back leave a destination the shell way. On the root it falls through to the activity's
     * own Back, so a launcher screen still moves its task to the back instead of being destroyed.
     */
    public static void installBack(ComponentActivity activity, Destination current) {
        activity.getOnBackPressedDispatcher().addCallback(activity, new OnBackPressedCallback(true) {
            @Override
            public void handleOnBackPressed() {
                if (current == root(activity)) {
                    // Let the activity's own Back run, then take Back over again for the next press.
                    setEnabled(false);
                    activity.getOnBackPressedDispatcher().onBackPressed();
                    setEnabled(true);
                } else {
                    leave(activity, current);
                }
            }
        });
    }
}
