package com.limelight.console;

import android.content.Context;
import android.content.res.Configuration;
import android.graphics.Canvas;
import android.graphics.Rect;
import android.view.View;

import androidx.recyclerview.widget.RecyclerView;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.CoverCard;

import java.util.ArrayList;
import java.util.List;
import java.util.function.Consumer;

/**
 * The hero at the top of Home on a phone and a handheld: which games it holds and what its Play does. The games to continue;
 * without any the favourites, without any the first games of the library; with no game at all there is no hero.
 */
final class HomeHero {
    static final String SHELF = "shelf:hero";
    /** How many games a hero holds when it borrows them from the library. */
    private static final int FROM_LIBRARY = 5;
    /** The middle cover's share of the width and the height it may take of the window (the design's 290 of 412 by 915). */
    private static final float COVER_OF_WIDTH = 290f / 412f;
    private static final float COVER_OF_HEIGHT = 435f / 915f;
    /** A neighbour against the middle cover, how far its centre is, and where it scales from so its top sits 30 lower. */
    static final float NEIGHBOUR_SCALE = 232f / 290f;
    private static final float PITCH_OF_COVER = 258f / 290f;
    private static final float PIVOT_OF_HEIGHT = 150f / 435f;

    enum Source { CONTINUE, FAVOURITES, LIBRARY }

    final List<HomeModel.GameCard> games;
    final Source source;

    HomeHero(List<HomeModel.GameCard> games, Source source) {
        this.games = games;
        this.source = source;
    }

    /** The hero of this library, or null when it has no game to show. */
    static HomeHero pick(HomeModel model, HomeView.Group group) {
        if (!model.continuePlaying.isEmpty()) return new HomeHero(model.continuePlaying, Source.CONTINUE);
        if (!model.favourites.isEmpty()) return new HomeHero(model.favourites, Source.FAVOURITES);
        for (HomeModel.Row row : model.rows(group)) {
            List<HomeModel.GameCard> games = new ArrayList<>();
            for (HomeModel.GameCard card : row.cards) {
                if (!card.notAdded() && games.size() < FROM_LIBRARY) games.add(card);
            }
            if (!games.isEmpty()) return new HomeHero(games, Source.LIBRARY);
        }
        return null;
    }

    /**
     * The game to show after a refresh. A game that newly leads takes the hero: it has just started running, or the first read
     * of the PC found it, and it is what the screen is for. Otherwise the hero stays where the person left it.
     */
    String shownAfterRefresh(String shown, String lastLead) {
        String lead = games.get(0).key;
        return lead.equals(lastLead) ? shown : lead;
    }

    int indexOf(String key) {
        for (int i = 0; key != null && i < games.size(); i++) {
            if (games.get(i).key.equals(key)) return i;
        }
        return 0;
    }

    /** Whether a press on Play needs the game's page: it adds, installs, wakes or asks which PC there. */
    static boolean opensPage(HomeModel.GameCard card, ComputerDetails host) {
        if (card.notAdded() || card.notInstalled() || host == null || host.state != ComputerDetails.State.ONLINE
                || !host.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
            return true;
        }
        return card.hostCount > 1 && card.badge == HomeModel.Badge.NONE;
    }

    /** A running or suspended game resumes; Play says so. */
    static int playLabel(HomeModel.GameCard card) {
        return card.badge == HomeModel.Badge.NONE ? R.string.library_play : R.string.applist_menu_resume;
    }

    /** The middle cover's width in this window; its height is 3/2 of it. */
    static int coverWidth(int windowWidthPx, int windowHeightPx) {
        return Math.round(Math.min(windowWidthPx * COVER_OF_WIDTH, windowHeightPx * COVER_OF_HEIGHT / 1.5f));
    }

    static int coverWidth(Context context) {
        Configuration config = context.getResources().getConfiguration();
        float density = context.getResources().getDisplayMetrics().density;
        return coverWidth(Math.round(config.screenWidthDp * density), Math.round(config.screenHeightDp * density));
    }

    /**
     * How far a cover is from the middle in steps of the pitch, from 0 in the middle to 1 at a neighbour's place and past it;
     * the scale and how far it stepped back follow from this alone.
     */
    static float stepsFromMiddle(float centre, float middle, float pitch) {
        return Math.min(1f, Math.abs(centre - middle) / pitch);
    }

    /**
     * The carousel's look: covers overlap by their decoration so a neighbour's edge peeks in, and each scales and steps back
     * with its distance from the middle. Written while the list draws, so it follows a fling frame by frame and costs
     * nothing at rest (an unchanged cover is not touched).
     */
    static final class Carousel extends RecyclerView.ItemDecoration {
        private final int overlap;
        private final int pitch;
        private final Consumer<Integer> onMiddle;
        private int middle = RecyclerView.NO_POSITION;

        Carousel(int coverWidth, Consumer<Integer> onMiddle) {
            pitch = Math.round(coverWidth * PITCH_OF_COVER);
            overlap = (coverWidth - pitch) / 2;
            this.onMiddle = onMiddle;
        }

        int pitch() {
            return pitch;
        }

        /** The side padding that lets the first and last cover stop in the middle. */
        int padding(int listWidth) {
            return Math.max(0, (listWidth - pitch) / 2);
        }

        /** The position counts as new again, for a list that changed under the carousel. */
        void forget() {
            middle = RecyclerView.NO_POSITION;
        }

        @Override
        public void getItemOffsets(Rect out, View view, RecyclerView parent, RecyclerView.State state) {
            out.set(-overlap, 0, -overlap, 0);
        }

        @Override
        public void onDraw(Canvas canvas, RecyclerView parent, RecyclerView.State state) {
            float centre = parent.getWidth() / 2f;
            int nearest = RecyclerView.NO_POSITION;
            float best = Float.MAX_VALUE;
            for (int i = 0; i < parent.getChildCount(); i++) {
                View child = parent.getChildAt(i);
                float at = (child.getLeft() + child.getRight()) / 2f + child.getTranslationX();
                float distance = Math.abs(at - centre);
                if (distance < best) {
                    best = distance;
                    nearest = parent.getChildAdapterPosition(child);
                }
                float steps = stepsFromMiddle(at, centre, pitch);
                child.setPivotX(child.getWidth() / 2f);
                child.setPivotY(child.getHeight() * PIVOT_OF_HEIGHT);
                if (child instanceof CoverCard) ((CoverCard) child).setRest(1f - steps * (1f - NEIGHBOUR_SCALE), steps);
            }
            if (nearest != RecyclerView.NO_POSITION && nearest != middle) {
                middle = nearest;
                onMiddle.accept(nearest);
            }
        }
    }
}
