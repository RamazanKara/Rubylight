package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.graphics.Bitmap;
import android.graphics.drawable.ColorDrawable;
import android.view.LayoutInflater;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.ImageView;

import com.limelight.R;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.GraphicsMode;
import org.robolectric.shadows.ShadowLooper;

/** What keeps a focus move cheap: no offscreen layer per dimmed cover, and only the two covers it concerns touched. */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = 30, application = Application.class)
public class CoverCardTest {
    private ActivityController<UiNavigationTest.Screen> controller;
    private CoverCard a;
    private CoverCard b;
    private CoverCard c;
    private View elsewhere;

    @Before
    public void attach() {
        controller = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible();
        FrameLayout root = new FrameLayout(controller.get());
        controller.get().setContentView(root);
        elsewhere = new View(controller.get());
        elsewhere.setFocusableInTouchMode(true);
        root.addView(elsewhere);
        CoverCard.Group group = new CoverCard.Group();
        a = card(root, group);
        b = card(root, group);
        c = card(root, group);
    }

    @After
    public void detach() {
        controller.close();
    }

    private CoverCard card(FrameLayout root, CoverCard.Group group) {
        CoverCard card = (CoverCard) LayoutInflater.from(root.getContext()).inflate(R.layout.console_card_cover, root, false);
        card.setFocusableInTouchMode(true);
        card.setGroup(group);
        root.addView(card);
        return card;
    }

    private static float dim(CoverCard card) {
        return card.findViewById(R.id.console_cover_frame).getAlpha();
    }

    @Test
    public void aDimmedCoverNeedsNoOffscreenLayer() {
        View frame = a.findViewById(R.id.console_cover_frame);
        assertTrue(frame instanceof CoverFrame);
        assertFalse("each dimmed cover would otherwise cost a render pass on each frame", frame.hasOverlappingRendering());
    }

    @Test
    public void aMoveBetweenCoversLeavesTheOthersAlone() {
        assertTrue(a.requestFocus());
        ShadowLooper.idleMainLooper();
        float stepped = dim(c);
        assertTrue(stepped < 1f);
        assertEquals(1f, dim(a), 0f);

        assertTrue(elsewhere.requestFocus());
        // The move's other half has not come yet: every cover undimming here would be redone a moment later.
        assertEquals(stepped, dim(c), 0f);

        assertTrue(b.requestFocus());
        ShadowLooper.idleMainLooper();
        assertEquals(1f, dim(b), 0f);
        assertEquals(stepped, dim(a), 0f);
        assertEquals(stepped, dim(c), 0f);
    }

    @Test
    public void focusLeavingTheCoversBringsThemBack() {
        assertTrue(a.requestFocus());
        ShadowLooper.idleMainLooper();
        assertTrue(elsewhere.requestFocus());
        ShadowLooper.idleMainLooper();
        assertEquals(1f, dim(b), 0f);
        assertEquals(1f, dim(c), 0f);
    }

    @Test
    public void aFocusedCoverRemovedFromTheScreenDoesNotLeaveTheOthersDimmed() {
        assertTrue(a.requestFocus());
        ShadowLooper.idleMainLooper();
        ((FrameLayout) a.getParent()).removeView(a);
        ShadowLooper.idleMainLooper();
        assertEquals(1f, dim(c), 0f);
    }

    @Test
    public void thePlaceholderIsPaintedOnlyWhereNoOpaqueArtHidesIt() {
        PosterImageView art = a.findViewById(R.id.console_cover);
        assertTrue("no art yet", art.needsPlaceholder());

        Bitmap opaque = Bitmap.createBitmap(30, 45, Bitmap.Config.ARGB_8888);
        opaque.eraseColor(0xFF336699);
        opaque.setHasAlpha(false);
        art.setImageBitmap(opaque);
        assertFalse("an opaque cover fills the frame", art.needsPlaceholder());

        art.setScaleType(ImageView.ScaleType.CENTER_INSIDE);
        assertTrue("an icon in the middle does not", art.needsPlaceholder());

        art.setScaleType(ImageView.ScaleType.CENTER_CROP);
        Bitmap seeThrough = Bitmap.createBitmap(30, 45, Bitmap.Config.ARGB_8888);
        art.setImageBitmap(seeThrough);
        assertTrue("art with transparency sits on it", art.needsPlaceholder());

        art.setImageDrawable(new ColorDrawable(0x80FF0000));
        assertTrue(art.needsPlaceholder());
    }

    @Test
    public void aShadowStopsWhereItNoLongerShows() {
        // Three sigmas (60 here) filled a third more pixels with what no eye can tell from nothing.
        assertEquals(45, GlassShadow.reachFor(40f));
    }
}
