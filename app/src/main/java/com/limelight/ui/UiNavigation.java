package com.limelight.ui;

import android.app.Activity;
import android.content.Intent;
import android.view.KeyEvent;
import android.view.View;
import android.widget.GridView;
import android.widget.ListView;

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
        boolean findLibrary = control && key == KeyEvent.KEYCODE_F && activity instanceof AppView;
        boolean find = findLibrary || (control && key == KeyEvent.KEYCODE_F && activity instanceof StreamSettings);
        if (!back && !help && !find && !(control && (key == KeyEvent.KEYCODE_N || key == KeyEvent.KEYCODE_COMMA))) {
            return false;
        }
        if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) {
            if (back) activity.onBackPressed();
            else if (help) {
                if (!(activity instanceof com.limelight.HelpTopicsActivity)) HelpLauncher.showHelp(activity);
            }
            else if (find) {
                if (findLibrary) ((AppView) activity).openLibrarySearch();
                else ((StreamSettings) activity).openSearch();
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
            // Menu, Shift+F10 and a controller's X open the selected card's options, like a long press.
            if (key != KeyEvent.KEYCODE_MENU && key != KeyEvent.KEYCODE_BUTTON_X
                    && !(key == KeyEvent.KEYCODE_F10 && event.hasModifiers(KeyEvent.META_SHIFT_ON))) {
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
            // A disabled row's explanation says why it is unavailable, so it opens there too.
            if (!(explanation instanceof Runnable)) return false;
            if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) ((Runnable) explanation).run();
            return true;
        });
    }

    /** PreferenceFragment replaces the key listener before attaching its first adapter. */
    public static void bindPreferencesWhenReady(ListView list) {
        class Binding implements android.view.ViewTreeObserver.OnGlobalLayoutListener, View.OnAttachStateChangeListener {
            private void remove() {
                list.getViewTreeObserver().removeOnGlobalLayoutListener(this);
                list.removeOnAttachStateChangeListener(this);
            }
            @Override public void onGlobalLayout() {
                if (list.getAdapter() == null) return;
                remove();
                bindPreferences(list);
            }
            @Override public void onViewAttachedToWindow(View view) { }
            @Override public void onViewDetachedFromWindow(View view) { remove(); }
        }
        Binding binding = new Binding();
        list.addOnAttachStateChangeListener(binding);
        list.getViewTreeObserver().addOnGlobalLayoutListener(binding);
    }
}
