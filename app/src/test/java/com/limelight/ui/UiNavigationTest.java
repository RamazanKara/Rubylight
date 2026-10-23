package com.limelight.ui;

import android.app.Application;
import android.content.Intent;
import android.os.Bundle;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ArrayAdapter;
import android.widget.BaseAdapter;
import android.widget.FrameLayout;
import android.widget.GridView;
import android.widget.ListView;

import androidx.appcompat.app.AppCompatActivity;

import com.limelight.R;
import com.limelight.preferences.AddComputerManually;
import com.limelight.preferences.ControllerMappingActivity;
import com.limelight.preferences.StreamSettings;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;

import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class UiNavigationTest {
    public static class Screen extends AppCompatActivity {
        @Override protected void onCreate(Bundle state) {
            setTheme(R.style.AppTheme);
            super.onCreate(state);
            setContentView(new FrameLayout(this));
        }
    }

    private static void layout(AppCompatActivity activity) {
        View root = activity.findViewById(android.R.id.content);
        root.measure(View.MeasureSpec.makeMeasureSpec(1000, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(800, View.MeasureSpec.EXACTLY));
        root.layout(0, 0, 1000, 800);
    }

    @Test
    public void computerCardLeavesTapsAndLongPressesToTheGrid() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup().visible();
        try {
            Screen screen = controller.get();
            View card = screen.getLayoutInflater().inflate(R.layout.pc_grid_item, new FrameLayout(screen), false);
            // Chip turns itself focusable when its text is set; GridView ignores item touches on cards with a focusable child.
            ((android.widget.TextView) card.findViewById(R.id.grid_status)).setText(R.string.pc_needs_pairing);
            assertFalse(card.hasExplicitFocusable());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void computerUpdateNoticeDoesNotOverlapTheNextGridRow() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup().visible();
        try {
            Screen screen = controller.get();
            android.content.SharedPreferences updates = screen.getSharedPreferences("host_update_state", 0);
            updates.edit().putString("latest_version", "2.5.11").apply();
            com.limelight.grid.PcGridAdapter adapter = new com.limelight.grid.PcGridAdapter(screen,
                    new com.limelight.preferences.PreferenceConfiguration());
            for (String name : new String[] {"Desk", "Laptop", "Studio"}) {
                com.limelight.nvstream.http.ComputerDetails details = new com.limelight.nvstream.http.ComputerDetails();
                details.name = name;
                details.state = com.limelight.nvstream.http.ComputerDetails.State.OFFLINE;
                if (name.equals("Desk")) {
                    details.caps = org.robolectric.util.ReflectionHelpers.callStaticMethod(
                            com.limelight.nvstream.http.HostCapabilities.class, "fromServerInfo",
                            org.robolectric.util.ReflectionHelpers.ClassParameter.from(String.class,
                                    "<root><RustHostVersion>2.5.10</RustHostVersion></root>"));
                }
                adapter.addComputer(new com.limelight.PcView.ComputerObject(details));
            }
            GridView grid = new GridView(screen);
            grid.setNumColumns(2);
            grid.setVerticalSpacing(8);
            grid.setAdapter(adapter);
            screen.setContentView(grid);
            layout(screen);
            assertEquals(3, grid.getChildCount());
            assertTrue("the next row clears the card with an update", grid.getChildAt(2).getTop() >=
                    Math.max(grid.getChildAt(0).getBottom(), grid.getChildAt(1).getBottom()) + 8);
            assertTrue(grid.getChildAt(0).getContentDescription().toString().contains("2.5.11"));
            assertFalse(grid.getChildAt(1).getContentDescription().toString().contains("2.5.11"));

            updates.edit().clear().apply();
            adapter.notifyDataSetChanged();
            layout(screen);
            assertEquals(View.GONE, grid.getChildAt(0).findViewById(R.id.grid_update).getVisibility());
            assertEquals(View.GONE, grid.getChildAt(1).findViewById(R.id.grid_update).getVisibility());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void manualHostEntryHasMatchingKeyboardAndAccessibilityOrder() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup().visible();
        try {
            Screen screen = controller.get();
            screen.setContentView(R.layout.activity_add_computer_manually);
            TopAppBar.setNavigation(TopAppBar.configure(screen, R.id.add_pc_toolbar, "Add PC", 0),
                    R.drawable.ic_close, android.R.string.cancel, R.id.add_pc_back, v -> { });
            layout(screen);
            View back = screen.findViewById(R.id.add_pc_back);
            View address = screen.findViewById(R.id.hostTextView);
            View add = screen.findViewById(R.id.addPcButton);
            assertSame(address, back.focusSearch(View.FOCUS_FORWARD));
            assertSame(add, address.focusSearch(View.FOCUS_FORWARD));
            assertSame(address, add.focusSearch(View.FOCUS_UP));
            assertEquals(back.getId(), address.getAccessibilityTraversalAfter());
            assertEquals(address.getId(), add.getAccessibilityTraversalAfter());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void emptyLibraryAndAppGridCanReturnToTheirHeaders() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup().visible();
        try {
            Screen screen = controller.get();
            screen.setContentView(R.layout.activity_pc_view);
            layout(screen);
            View add = screen.findViewById(R.id.discovery_add);
            View help = screen.findViewById(R.id.discovery_help);
            assertSame(help, add.focusSearch(View.FOCUS_DOWN));
            assertSame(add, help.focusSearch(View.FOCUS_UP));
            assertEquals(add.getId(), help.getAccessibilityTraversalAfter());

            screen.setContentView(R.layout.activity_app_view);
            TopAppBar.setUpNavigation(TopAppBar.configure(screen, R.id.library_toolbar, "PC", R.menu.library),
                    R.id.library_back, v -> { });
            FrameLayout container = screen.findViewById(R.id.appFragmentContainer);
            screen.getLayoutInflater().inflate(R.layout.app_grid_view, container);
            GridView grid = screen.findViewById(R.id.fragmentView);
            grid.setAdapter(new ArrayAdapter<>(screen, android.R.layout.simple_list_item_1,
                    new String[] {"One", "Two", "Three"}));
            layout(screen);
            View back = screen.findViewById(R.id.library_back);
            assertSame(grid, back.focusSearch(View.FOCUS_DOWN));
            assertSame(back, grid.focusSearch(View.FOCUS_UP));
            assertEquals(back.getId(), grid.getAccessibilityTraversalAfter());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void shortcutsRunOnReleaseAndLeaveOrdinaryKeysAlone() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        try {
            Screen screen = controller.get();
            assertFalse(UiNavigation.handleKey(screen, new KeyEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_N)));
            assertTrue(UiNavigation.handleKey(screen, key(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_N, KeyEvent.META_CTRL_ON)));
            assertNull(Shadows.shadowOf(screen).getNextStartedActivity());
            assertTrue(UiNavigation.handleKey(screen, key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_N, KeyEvent.META_CTRL_ON)));
            Intent add = Shadows.shadowOf(screen).getNextStartedActivity();
            assertEquals(AddComputerManually.class.getName(), add.getComponent().getClassName());
            UiNavigation.handleKey(screen, key(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_COMMA, KeyEvent.META_CTRL_ON));
            assertEquals(StreamSettings.class.getName(), Shadows.shadowOf(screen).getNextStartedActivity().getComponent().getClassName());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void f1OpensTheBundledHelp() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        try {
            Screen screen = controller.get();
            assertTrue(UiNavigation.handleKey(screen, new KeyEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_F1)));
            assertEquals(com.limelight.HelpTopicsActivity.class.getName(),
                    Shadows.shadowOf(screen).getNextStartedActivity().getComponent().getClassName());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void preferenceExplanationDoesNotStealSelectionOrToggleTheSwitch() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup().visible();
        try {
            Screen screen = controller.get();
            ListView list = new ListView(screen);
            View row = screen.getLayoutInflater().inflate(R.layout.settings_preference, list, false);
            ViewGroup widget = row.findViewById(android.R.id.widget_frame);
            screen.getLayoutInflater().inflate(R.layout.settings_switch, widget);
            AtomicInteger helpCount = new AtomicInteger();
            AtomicInteger toggles = new AtomicInteger();
            row.setTag(R.id.preference_help, (Runnable) helpCount::incrementAndGet);
            list.setAdapter(new BaseAdapter() {
                @Override public int getCount() { return 1; }
                @Override public Object getItem(int position) { return position; }
                @Override public long getItemId(int position) { return position; }
                @Override public View getView(int position, View recycled, ViewGroup parent) { return row; }
            });
            list.setOnItemClickListener((parent, view, position, id) -> toggles.incrementAndGet());
            UiNavigation.bindPreferences(list);
            screen.setContentView(list);
            list.requestFocus();
            list.setSelection(0);
            layout(screen);
            assertTrue(list.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_RIGHT)));
            assertTrue(list.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_RIGHT)));
            assertEquals(1, helpCount.get());
            assertEquals(0, toggles.get());
            assertEquals(0, list.getSelectedItemPosition());
            assertEquals(ViewGroup.FOCUS_BLOCK_DESCENDANTS, ((ViewGroup) row).getDescendantFocusability());
            list.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DPAD_CENTER));
            list.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_DPAD_CENTER));
            assertEquals(1, toggles.get());
            // A disabled row still explains itself, which says why it is unavailable.
            row.setEnabled(false);
            assertTrue(list.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_F1)));
            assertTrue(list.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_F1)));
            assertEquals(2, helpCount.get());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void controllerMappingRequiresExplicitCaptureAndCanCancelIt() {
        ActivityController<ControllerMappingActivity> controller = Robolectric.buildActivity(ControllerMappingActivity.class).setup();
        try {
            ControllerMappingActivity screen = controller.get();
            KeyEvent button = new KeyEvent(0, 0, KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BUTTON_X,
                    0, 0, 0, 0, 0, InputDevice.SOURCE_GAMEPAD);
            screen.dispatchKeyEvent(button);
            assertNull(ShadowDialog.getLatestDialog());
            screen.findViewById(R.id.controller_mapping_capture).performClick();
            screen.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_BACK));
            assertFalse(screen.isFinishing());
            screen.dispatchKeyEvent(button);
            assertNull(ShadowDialog.getLatestDialog());
            screen.findViewById(R.id.controller_mapping_capture).performClick();
            screen.dispatchKeyEvent(button);
            assertTrue(ShadowDialog.getLatestDialog().isShowing());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void actionSheetFocusMovesInTheSameOrderAsItsActions() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        ActionSheet sheet = new ActionSheet(controller.get(), "Actions");
        View first = sheet.addAction(R.drawable.ic_play, "Play");
        View second = sheet.addAction(R.drawable.ic_settings, "Settings");
        try {
            sheet.show();
            View decor = sheet.getWindow().getDecorView();
            decor.measure(View.MeasureSpec.makeMeasureSpec(600, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(800, View.MeasureSpec.EXACTLY));
            decor.layout(0, 0, 600, 800);
            assertTrue(first.hasFocus());
            assertSame(second, first.focusSearch(View.FOCUS_DOWN));
            assertSame(first, second.focusSearch(View.FOCUS_UP));
        } finally {
            sheet.dismiss();
            controller.pause().stop().destroy();
        }
    }

    private static AdaptiveGridView computerGrid(Screen screen, AtomicInteger menus) {
        com.limelight.grid.PcGridAdapter adapter = new com.limelight.grid.PcGridAdapter(screen,
                new com.limelight.preferences.PreferenceConfiguration());
        for (String name : new String[] {"Desk", "Studio"}) {
            com.limelight.nvstream.http.ComputerDetails details = new com.limelight.nvstream.http.ComputerDetails();
            details.name = name;
            details.state = com.limelight.nvstream.http.ComputerDetails.State.OFFLINE;
            adapter.addComputer(new com.limelight.PcView.ComputerObject(details));
        }
        AdaptiveGridView grid = (AdaptiveGridView) screen.getLayoutInflater().inflate(R.layout.pc_grid_view,
                (ViewGroup) screen.findViewById(R.id.pcFragmentContainer), false);
        ((ViewGroup) screen.findViewById(R.id.pcFragmentContainer)).addView(grid);
        grid.setColumnPolicy(true, false);
        grid.setAdapter(adapter);
        grid.setOnItemLongClickListener((parent, view, position, id) -> {
            menus.incrementAndGet();
            return true;
        });
        return grid;
    }

    private static void press(View view, int key) {
        view.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, key));
        view.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, key));
    }

    @Test
    public void dpadReachesTheShellBarOnATouchscreenPhone() {
        // AppCompat's toolbar blocks focus on touchscreen devices; the shell bar must not.
        org.robolectric.Shadows.shadowOf(org.robolectric.RuntimeEnvironment.getApplication().getPackageManager())
                .setSystemFeature(android.content.pm.PackageManager.FEATURE_TOUCHSCREEN, true);
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup().visible();
        try {
            Screen screen = controller.get();
            screen.setContentView(R.layout.activity_pc_view);
            ShellBar.install(screen, ShellBar.Destination.PCS);
            screen.findViewById(R.id.no_pc_found_layout).setVisibility(View.GONE);
            AdaptiveGridView grid = computerGrid(screen, new AtomicInteger());
            layout(screen);
            grid.requestFocus();
            grid.setSelection(0);
            View up = grid.focusSearch(View.FOCUS_UP);
            assertNotNull("UP from the PC list found nothing", up);
            assertSame(screen.findViewById(R.id.shell_bar_items), up.getParent());
            View settings = screen.findViewById(R.id.shell_settings);
            assertTrue(settings.isFocusable());
            assertSame(grid, settings.focusSearch(View.FOCUS_DOWN));
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void dpadStepsOntoACardsOptionsButtonAndOpensItsMenu() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup().visible();
        try {
            Screen screen = controller.get();
            screen.setContentView(R.layout.activity_pc_view);
            AtomicInteger menus = new AtomicInteger();
            AdaptiveGridView grid = computerGrid(screen, menus);
            layout(screen);
            grid.requestFocus();
            // The last card: no neighbour to its right, so the options button is next.
            grid.setSelection(1);
            layout(screen);
            View more = grid.getSelectedView().findViewById(R.id.grid_more);
            press(grid, KeyEvent.KEYCODE_DPAD_RIGHT);
            assertTrue(grid.isActionMarked());
            assertTrue("the options button shows the focus ring", more.isHovered());
            assertEquals(1, grid.getSelectedItemPosition());
            press(grid, KeyEvent.KEYCODE_DPAD_CENTER);
            assertEquals(1, menus.get());
            press(grid, KeyEvent.KEYCODE_DPAD_LEFT);
            assertFalse(grid.isActionMarked());
            assertFalse(more.isHovered());
            press(grid, KeyEvent.KEYCODE_DPAD_RIGHT);
            press(grid, KeyEvent.KEYCODE_DPAD_DOWN);
            assertFalse("moving on leaves the options button", grid.isActionMarked());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void dpadMovesCardToCardInOnePressAndMarksOptionsOnlyAtTheRowEnd() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup().visible();
        try {
            Screen screen = controller.get();
            screen.setContentView(R.layout.activity_pc_view);
            AdaptiveGridView grid = computerGrid(screen, new AtomicInteger());
            layout(screen);
            grid.setNumColumns(2);
            layout(screen);
            grid.requestFocus();
            grid.setSelection(0);
            layout(screen);
            press(grid, KeyEvent.KEYCODE_DPAD_RIGHT);
            assertFalse("a card with a neighbour does not stop on its options", grid.isActionMarked());
            assertEquals(1, grid.getSelectedItemPosition());
            press(grid, KeyEvent.KEYCODE_DPAD_RIGHT);
            assertTrue("the last card in the row steps onto its options", grid.isActionMarked());
            assertEquals(1, grid.getSelectedItemPosition());
            press(grid, KeyEvent.KEYCODE_DPAD_LEFT);
            assertFalse(grid.isActionMarked());
            press(grid, KeyEvent.KEYCODE_DPAD_LEFT);
            assertEquals(0, grid.getSelectedItemPosition());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test
    public void mouseHoverHighlightsCardsWithoutTakingKeyboardFocus() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup();
        try {
            Screen screen = controller.get();
            com.limelight.grid.PcGridAdapter adapter = new com.limelight.grid.PcGridAdapter(screen,
                    new com.limelight.preferences.PreferenceConfiguration());
            com.limelight.nvstream.http.ComputerDetails details = new com.limelight.nvstream.http.ComputerDetails();
            details.name = "PC";
            details.state = com.limelight.nvstream.http.ComputerDetails.State.OFFLINE;
            adapter.addComputer(new com.limelight.PcView.ComputerObject(details));
            View card = adapter.getView(0, null, new GridView(screen));
            android.view.MotionEvent hover = android.view.MotionEvent.obtain(0, 0,
                    android.view.MotionEvent.ACTION_HOVER_ENTER, 10, 10, 0);
            hover.setSource(InputDevice.SOURCE_MOUSE);
            card.dispatchGenericMotionEvent(hover);
            assertTrue(card.isHovered());
            assertFalse(card.hasFocus());
            hover.setAction(android.view.MotionEvent.ACTION_HOVER_EXIT);
            card.dispatchGenericMotionEvent(hover);
            assertFalse(card.isHovered());
            hover.recycle();
        } finally {
            controller.pause().stop().destroy();
        }
    }

    @Test @Config(qualifiers = "television")
    public void tvFocusMovesBetweenCardsAndBackLeavesTheOptionsButton() {
        try (var controller = Robolectric.buildActivity(Screen.class).setup().visible()) {
            Screen screen = controller.get();
            screen.setContentView(R.layout.activity_pc_view);
            AtomicInteger menus = new AtomicInteger();
            AdaptiveGridView grid = computerGrid(screen, menus);
            layout(screen);
            grid.requestFocus();
            grid.setSelection(0);
            layout(screen);
            View first = grid.getChildAt(0);
            first.jumpDrawablesToCurrentState();
            assertEquals(1.04f, first.getScaleX(), 0.001f);
            press(grid, KeyEvent.KEYCODE_DPAD_RIGHT);
            layout(screen);
            first.jumpDrawablesToCurrentState();
            grid.getSelectedView().jumpDrawablesToCurrentState();
            assertEquals(1f, first.getScaleX(), 0.001f);
            assertEquals(1.04f, grid.getSelectedView().getScaleX(), 0.001f);
            for (int key : new int[]{KeyEvent.KEYCODE_BACK, KeyEvent.KEYCODE_BUTTON_B, KeyEvent.KEYCODE_ESCAPE}) {
                press(grid, KeyEvent.KEYCODE_DPAD_RIGHT);
                assertTrue(grid.isActionMarked());
                press(grid, key);
                assertFalse(grid.isActionMarked());
                assertEquals(1, grid.getSelectedItemPosition());
            }
            assertEquals(0, menus.get());
        }
    }

    @Test
    public void computerAccessibilityActionOpensTheSameMenuAsLongPress() {
        ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup().visible();
        try {
            Screen screen = controller.get();
            com.limelight.grid.PcGridAdapter adapter = new com.limelight.grid.PcGridAdapter(screen,
                    new com.limelight.preferences.PreferenceConfiguration());
            com.limelight.nvstream.http.ComputerDetails details = new com.limelight.nvstream.http.ComputerDetails();
            details.name = "PC";
            details.state = com.limelight.nvstream.http.ComputerDetails.State.OFFLINE;
            adapter.addComputer(new com.limelight.PcView.ComputerObject(details));
            GridView grid = new GridView(screen);
            grid.setAdapter(adapter);
            AtomicInteger menus = new AtomicInteger();
            grid.setOnItemLongClickListener((parent, view, position, id) -> {
                assertEquals(0, position);
                menus.incrementAndGet();
                return true;
            });
            screen.setContentView(grid);
            layout(screen);
            View card = grid.getChildAt(0);
            android.view.accessibility.AccessibilityNodeInfo info = card.createAccessibilityNodeInfo();
            assertTrue(info.getActionList().stream().anyMatch(action ->
                    action.getId() == android.view.accessibility.AccessibilityNodeInfo.ACTION_LONG_CLICK &&
                    screen.getString(R.string.pc_accessibility_actions).contentEquals(action.getLabel())));
            assertTrue(card.performAccessibilityAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_LONG_CLICK, null));
            assertEquals(1, menus.get());
        } finally {
            controller.pause().stop().destroy();
        }
    }

    private static KeyEvent key(int action, int key, int modifiers) {
        return new KeyEvent(0, 0, action, key, 0, modifiers);
    }
}
