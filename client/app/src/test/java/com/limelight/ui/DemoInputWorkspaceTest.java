package com.limelight.ui;

import android.app.Application;
import android.os.Bundle;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;

import com.limelight.demo.DemoInputWorkspaceActivity;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, qualifiers = "w960dp-h600dp-land-mdpi")
public class DemoInputWorkspaceTest {
    @Test public void previewKeysEditLocalNoteAndHiddenControlsCanReopen() {
        try (ActivityController<DemoInputWorkspaceActivity> controller =
                     Robolectric.buildActivity(DemoInputWorkspaceActivity.class).setup()) {
            DemoInputWorkspaceActivity activity = controller.get();
            assertEquals(DesktopControlsPanel.Mode.COMBINED, activity.controls().getMode());
            EditText note = (EditText) described(activity.findViewById(android.R.id.content), "Editable preview note");
            assertNotNull(note);
            note.setText("Original note");
            note.setSelection(note.length());
            Object desktop = ReflectionHelpers.getField(activity, "desktop");
            ReflectionHelpers.callInstanceMethod(desktop, "sendKeys",
                    ReflectionHelpers.ClassParameter.from(int[].class,
                            new int[] {KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_A}));
            ReflectionHelpers.callInstanceMethod(desktop, "sendKeys",
                    ReflectionHelpers.ClassParameter.from(int[].class, new int[] {KeyEvent.KEYCODE_H}));
            assertEquals("h", note.getText().toString());

            View hide = described(activity.controls(), "Hide desktop controls");
            assertNotNull(hide);
            hide.performClick();
            assertEquals(View.GONE, activity.controls().getVisibility());
            described(activity.findViewById(android.R.id.content), "Show desktop controls").performClick();
            assertEquals(View.VISIBLE, activity.controls().getVisibility());
        }
    }

    @Test public void activityRecreationRetainsNoteAndChosenMode() {
        Bundle saved = new Bundle();
        try (ActivityController<DemoInputWorkspaceActivity> controller =
                     Robolectric.buildActivity(DemoInputWorkspaceActivity.class).setup()) {
            DemoInputWorkspaceActivity activity = controller.get();
            ((EditText) described(activity.findViewById(android.R.id.content), "Editable preview note")).setText("Keep this draft");
            activity.controls().setMode(DesktopControlsPanel.Mode.TRACKPAD);
            controller.saveInstanceState(saved);
        }
        try (ActivityController<DemoInputWorkspaceActivity> restored =
                     Robolectric.buildActivity(DemoInputWorkspaceActivity.class).create(saved).start().resume().visible()) {
            assertEquals(DesktopControlsPanel.Mode.TRACKPAD, restored.get().controls().getMode());
            assertEquals("Keep this draft", ((EditText) described(restored.get().findViewById(android.R.id.content),
                    "Editable preview note")).getText().toString());
        }
    }

    private static View described(View view, String description) {
        if (description.contentEquals(view.getContentDescription() == null ? "" : view.getContentDescription())) return view;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                View found = described(group.getChildAt(i), description);
                if (found != null) return found;
            }
        }
        return null;
    }
}
