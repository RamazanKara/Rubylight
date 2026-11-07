package com.limelight.ui;

import android.view.View;

import androidx.core.content.res.ResourcesCompat;
import androidx.dynamicanimation.animation.DynamicAnimation;
import androidx.dynamicanimation.animation.SpringAnimation;
import androidx.dynamicanimation.animation.SpringForce;

import com.limelight.R;

/**
 * The app's springs, from the motion tokens. Every target change retargets the running spring from where it is and at
 * its speed, so nothing waits for an animation to end. With Reduce motion a spring is a jump.
 */
public final class Springs {
    private Springs() { }

    /** A token pair. */
    public enum Kind {
        PRESS(R.dimen.nj_motion_press_stiffness, R.dimen.nj_motion_press_ratio),
        FOCUS(R.dimen.nj_motion_focus_stiffness, R.dimen.nj_motion_focus_ratio),
        TAB_PILL(R.dimen.nj_motion_tab_pill_stiffness, R.dimen.nj_motion_tab_pill_ratio);

        final int stiffness;
        final int ratio;

        Kind(int stiffness, int ratio) {
            this.stiffness = stiffness;
            this.ratio = ratio;
        }
    }

    /** A new force for a token pair, its final position set by the animation. */
    public static SpringForce force(View view, Kind kind) {
        return new SpringForce()
                .setStiffness(ResourcesCompat.getFloat(view.getResources(), kind.stiffness))
                .setDampingRatio(ResourcesCompat.getFloat(view.getResources(), kind.ratio));
    }

    /** Springs the view's scale to {@code target}, from wherever it is now. */
    public static void scale(View view, float target, Kind kind) {
        to(view, DynamicAnimation.SCALE_X, R.id.spring_scale_x, target, kind);
        to(view, DynamicAnimation.SCALE_Y, R.id.spring_scale_y, target, kind);
    }

    /** Lands any running scale spring at its target now. */
    public static void finish(View view) {
        land(view, DynamicAnimation.SCALE_X, R.id.spring_scale_x);
        land(view, DynamicAnimation.SCALE_Y, R.id.spring_scale_y);
    }

    /** Not skipToEnd: that takes effect on the next frame, and a held key must show the new card now. */
    private static void land(View view, DynamicAnimation.ViewProperty property, int tag) {
        SpringAnimation spring = (SpringAnimation) view.getTag(tag);
        if (spring == null || !spring.isRunning()) return;
        spring.cancel();
        property.setValue(view, spring.getSpring().getFinalPosition());
    }

    private static void to(View view, DynamicAnimation.ViewProperty property, int tag, float target, Kind kind) {
        SpringAnimation spring = (SpringAnimation) view.getTag(tag);
        if (Motion.reduced(view.getContext())) {
            if (spring != null) spring.cancel();
            property.setValue(view, 1f);
            return;
        }
        if (spring == null) {
            spring = new SpringAnimation(view, property).setSpring(force(view, kind).setFinalPosition(target));
            view.setTag(tag, spring);
        } else {
            spring.getSpring().setStiffness(ResourcesCompat.getFloat(view.getResources(), kind.stiffness))
                    .setDampingRatio(ResourcesCompat.getFloat(view.getResources(), kind.ratio));
        }
        spring.animateToFinalPosition(target);
    }
}
