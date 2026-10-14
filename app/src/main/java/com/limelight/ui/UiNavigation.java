package com.limelight.ui;

import android.app.Activity;
import android.content.Intent;
import android.view.KeyEvent;
import android.view.View;
import android.widget.GridView;
import android.widget.ListView;

import androidx.appcompat.widget.SearchView;
import androidx.core.view.ViewCompat;

import com.limelight.R;
import com.limelight.AppView;
import com.limelight.preferences.AddComputerManually;
import com.limelight.preferences.StreamSettings;
import com.limelight.utils.HelpLauncher;

public final class UiNavigation {
    public static void install(Activity activity, View root) {
        ViewCompat.addOnUnhandledKeyEventListener(root, (view, event) -> handleKey(activity, event));
    }

    static boolean handleKey(Activity activity, KeyEvent event) {
        int key = event.getKeyCode();
        boolean control = event.hasModifiers(KeyEvent.META_CTRL_ON);
        boolean back = event.hasNoModifiers() && (key == KeyEvent.KEYCODE_ESCAPE || key == KeyEvent.KEYCODE_BUTTON_B);
        boolean help = event.hasNoModifiers() && key == KeyEvent.KEYCODE_F1;
        SearchView search = activity.findViewById(R.id.settings_search);
        boolean findLibrary = control && key == KeyEvent.KEYCODE_F && activity instanceof AppView;
        boolean find = findLibrary || (control && key == KeyEvent.KEYCODE_F && search != null && search.getVisibility() == View.VISIBLE);
        if (!back && !help && !find && !(control && (key == KeyEvent.KEYCODE_N || key == KeyEvent.KEYCODE_COMMA))) {
            return false;
        }
        if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) {
            if (back) activity.onBackPressed();
            else if (help) HelpLauncher.launchTroubleshooting(activity);
            else if (find) {
                if (findLibrary) ((AppView) activity).openLibrarySearch();
                else {
                    search.setIconified(false);
                    search.requestFocus();
                }
            } else if (key == KeyEvent.KEYCODE_N && !(activity instanceof AddComputerManually)) {
                activity.startActivity(new Intent(activity, AddComputerManually.class));
            } else if (key == KeyEvent.KEYCODE_COMMA && !(activity instanceof StreamSettings)) {
                activity.startActivity(new Intent(activity, StreamSettings.class));
            }
        }
        return true;
    }

    public static void bindGrid(GridView grid) {
        grid.setOnKeyListener((view, key, event) -> {
            if (key != KeyEvent.KEYCODE_MENU && !(key == KeyEvent.KEYCODE_F10 && event.hasModifiers(KeyEvent.META_SHIFT_ON))) {
                return false;
            }
            if (grid.getSelectedItemPosition() < 0 || grid.getOnItemLongClickListener() == null) return false;
            if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) {
                grid.getOnItemLongClickListener().onItemLongClick(grid, grid.getSelectedView(),
                        grid.getSelectedItemPosition(), grid.getSelectedItemId());
            }
            return true;
        });
    }

    public static void bindPreferences(ListView list) {
        list.setOnKeyListener((view, key, event) -> {
            int helpDirection = ViewCompat.getLayoutDirection(list) == ViewCompat.LAYOUT_DIRECTION_RTL ?
                    KeyEvent.KEYCODE_DPAD_LEFT : KeyEvent.KEYCODE_DPAD_RIGHT;
            if (key != helpDirection && key != KeyEvent.KEYCODE_F1) return false;
            View row = list.getSelectedView();
            Object explanation = row == null ? null : row.getTag(R.id.preference_help);
            if (!(explanation instanceof Runnable) || !row.isEnabled()) return false;
            if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) ((Runnable) explanation).run();
            return true;
        });
    }
}
