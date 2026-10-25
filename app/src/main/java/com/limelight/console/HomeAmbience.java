package com.limelight.console;

import android.content.Context;
import android.view.Choreographer;
import android.view.View;
import android.view.animation.AnimationUtils;
import android.widget.TextView;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.BackdropView;
import com.limelight.ui.Motion;

import java.util.function.BooleanSupplier;
import java.util.function.Function;
import java.util.function.Supplier;

/**
 * What TV Home shows about the card that has focus: its cover as the screen's backdrop and its name, state and keys in the
 * hero above the shelves. Focus moves in {@code onFocusChange}; this follows a moment later, and writes the hero's text once
 * per frame at most, so a held key costs neither a text layout nor an animation per card.
 */
final class HomeAmbience implements ShelfAdapter.FocusListener, Choreographer.FrameCallback {
    private final Context context;
    private final BackdropView backdrop;
    private final View hero;
    private final TextView meta;
    private final TextView title;
    private final TextView hints;
    private final Function<String, ComputerDetails> hosts;
    private final Supplier<CoverLoaders> covers;
    private final BooleanSupplier fast;

    private ShelfAdapter.Item pending;
    private String shown;
    private boolean queued;

    /** Either part may be missing (a layout without the backdrop or the hero, as on a phone). */
    HomeAmbience(Context context, BackdropView backdrop, View hero, Function<String, ComputerDetails> hosts,
                 Supplier<CoverLoaders> covers, BooleanSupplier fast) {
        this.context = context;
        this.backdrop = backdrop;
        this.hero = hero;
        this.meta = hero == null ? null : hero.findViewById(R.id.home_hero_meta);
        this.title = hero == null ? null : hero.findViewById(R.id.home_hero_title);
        this.hints = hero == null ? null : hero.findViewById(R.id.home_hero_hints);
        this.hosts = hosts;
        this.covers = covers;
        this.fast = fast;
    }

    @Override
    public void onCardFocus(String shelf, ShelfAdapter.Item item, View card) {
        select(item);
    }

    private void select(ShelfAdapter.Item item) {
        pending = item;
        if (!queued && hero != null) {
            queued = true;
            Choreographer.getInstance().postFrameCallback(this);
        }
        askBackdrop(item);
    }

    /** The focused card's state can change under it (it starts or stops running); write the hero again. */
    void refresh(ShelfAdapter.Item item) {
        if (item == null) return;
        shown = null;
        select(item);
    }

    /** The key of the game or PC the backdrop and hero follow now; null when none. */
    String current() {
        return pending == null ? null : pending.key;
    }

    /** A cover that was still loading has arrived: if it is the focused game's, the backdrop can have it. */
    void onArt(com.limelight.nvstream.http.NvApp app) {
        ShelfAdapter.Item item = pending;
        if (item != null && item.game != null && item.game.appId == app.getAppId()) askBackdrop(item);
    }

    private void askBackdrop(ShelfAdapter.Item item) {
        if (backdrop == null) return;
        if (item.game == null) {
            // A PC has no art: the canvas, with no waiting for a cover.
            backdrop.focus("host:" + item.host.uuid, null, true, fast.getAsBoolean());
            return;
        }
        HomeModel.GameCard card = item.game;
        CoverLoaders loaders = covers.get();
        ComputerDetails host = hosts.apply(card.hostUuid);
        com.limelight.nvstream.http.NvApp app = CoverLoaders.appOf(card.title, card.appId, card.source);
        backdrop.focus("game:" + card.hostUuid + ":" + card.appId, loaders == null ? null : loaders.peek(host, app),
                loaders != null && loaders.lacksArt(host, app), fast.getAsBoolean());
    }

    /** No card has focus (the shelves are empty). */
    void clear() {
        pending = null;
        shown = null;
        if (title != null) title.setText("");
        if (meta != null) meta.setText("");
        if (hints != null) hints.setText("");
    }

    @Override
    public void doFrame(long frameTimeNanos) {
        queued = false;
        ShelfAdapter.Item item = pending;
        if (item == null || hero == null) return;
        // The same card told twice (a refresh repeats its focus) costs no text layout.
        if (item.key.equals(shown) && !hasChanged(item)) return;
        boolean first = shown == null;
        shown = item.key;
        write(item);
        if (!first && !fast.getAsBoolean() && !Motion.reduced(context)) enter();
        else {
            hero.animate().cancel();
            hero.setAlpha(1f);
            hero.setTranslationX(0f);
        }
    }

    private String written;

    private boolean hasChanged(ShelfAdapter.Item item) {
        return !text(item).equals(written);
    }

    private String text(ShelfAdapter.Item item) {
        if (item.game != null) {
            HomeModel.GameCard card = item.game;
            CharSequence state = ShelfAdapter.CardHolder.stateOf(context, card);
            String where = ShelfAdapter.CardHolder.whereOf(context, card);
            return (state == null ? "" : state + " · ") + where + "\n" + card.title;
        }
        return ShelfAdapter.CardHolder.hostLine(context, item.host) + "\n" + item.host.name;
    }

    private void write(ShelfAdapter.Item item) {
        String options = context.getString(R.string.console_hint_options);
        if (item.game != null) {
            HomeModel.GameCard card = item.game;
            CharSequence state = ShelfAdapter.CardHolder.stateOf(context, card);
            String where = ShelfAdapter.CardHolder.whereOf(context, card);
            setText(meta, state == null ? where : state + " · " + where);
            setText(title, card.title);
            setText(hints, "A  " + context.getString(R.string.library_play) + "      X  " + options);
        } else {
            setText(meta, ShelfAdapter.CardHolder.hostLine(context, item.host));
            setText(title, item.host.name);
            setText(hints, "A  " + context.getString(R.string.library_open) + "      X  " + options);
        }
        written = text(item);
    }

    private static void setText(TextView view, CharSequence text) {
        if (view != null && !text.toString().contentEquals(view.getText())) view.setText(text);
    }

    /** The new text comes in from a little to the side over 140 ms. */
    private void enter() {
        float offset = 8 * context.getResources().getDisplayMetrics().density;
        hero.animate().cancel();
        hero.setAlpha(0f);
        hero.setTranslationX(offset);
        hero.animate().alpha(1f).translationX(0f).setDuration(140)
                .setInterpolator(AnimationUtils.loadInterpolator(context, R.interpolator.tv_emph_dec)).start();
    }
}
