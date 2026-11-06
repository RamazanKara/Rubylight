package com.limelight.console;

import android.content.Context;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.TextView;

import androidx.recyclerview.widget.PagerSnapHelper;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.CoverCard;
import com.limelight.ui.InputSource;
import com.limelight.ui.Motion;

import java.util.function.Consumer;
import java.util.function.Function;
import java.util.function.Supplier;

/**
 * A hero's views for its game. On a phone a carousel of covers with the one Play over the middle cover's foot; on a handheld
 * the cover, its chips, title and buttons, with the other games to continue in a panel beside them.
 */
abstract class HomeHeroViews {
    final MaterialButton play;
    final ConsoleHomeAdapter.Listener listener;
    final Context context;
    HomeModel.GameCard game;

    HomeHeroViews(View root, ConsoleHomeAdapter.Listener listener) {
        this.play = root.findViewById(R.id.hero_play);
        this.listener = listener;
        this.context = root.getContext();
        play.setOnClickListener(v -> {
            if (game != null) listener.onHeroPlay(game);
        });
        play.setOnLongClickListener(v -> {
            if (game != null) listener.onGameMenu(game);
            return game != null;
        });
        // The first key after a finger lands on Play, not on whatever comes first in the tree.
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) play.setFocusedByDefault(true);
        play.setOnFocusChangeListener((v, focused) -> {
            if (focused && game != null) listener.onHeroShown(game);
            listener.onHeroFocus(focused ? play.getText() : null);
        });
    }

    /** {@code animate}: the game changed under a press, so the new one comes in; a refresh just writes. */
    abstract void show(HomeHero hero, String key, boolean animate);

    /** A line of the panel: when it was last played, else where it is. */
    static CharSequence lineOf(Context context, HomeModel.GameCard card, long now) {
        if (card.lastPlayed != null && card.lastPlayed > 0) {
            // The host's time is in seconds.
            return PlayedWhen.of(context, card.lastPlayed * 1000L, now);
        }
        return ShelfAdapter.CardHolder.whereOf(context, card);
    }

    static HomeHeroViews phone(View root, RecyclerView cards, ShelfLayoutManager layout, ShelfAdapter adapter,
                               ConsoleHomeAdapter.Listener listener, Consumer<HomeModel.GameCard> onMiddle) {
        return new Phone(root, cards, layout, adapter, listener, onMiddle);
    }

    static HomeHeroViews wide(View root, Context context, ConsoleHomeAdapter.Listener listener, Supplier<CoverLoaders> covers,
                              Function<String, ComputerDetails> hosts) {
        return new Wide(root, listener, covers, hosts);
    }

    private static final class Phone extends HomeHeroViews {
        private final RecyclerView cards;
        private final ShelfLayoutManager layout;
        private final ShelfAdapter adapter;
        private final HomeHero.Carousel carousel;
        private final Consumer<HomeModel.GameCard> onMiddle;
        private HomeHero hero;

        Phone(View root, RecyclerView cards, ShelfLayoutManager layout, ShelfAdapter adapter,
              ConsoleHomeAdapter.Listener listener, Consumer<HomeModel.GameCard> onMiddle) {
            super(root, listener);
            this.cards = cards;
            this.layout = layout;
            this.adapter = adapter;
            this.onMiddle = onMiddle;
            int cover = HomeHero.coverWidth(cards.getContext());
            carousel = new HomeHero.Carousel(cover, this::middleAt);
            cards.addItemDecoration(carousel);
            layout.setCentred(true);
            cards.setOnFlingListener(null);
            new PagerSnapHelper().attachToRecyclerView(cards);
            // Centred from the first frame: the window's width now, corrected if the list turns out narrower.
            android.content.res.Resources res = cards.getResources();
            int window = Math.round(res.getConfiguration().screenWidthDp * res.getDisplayMetrics().density);
            int start = carousel.padding(window);
            cards.setPaddingRelative(start, cards.getPaddingTop(), start, cards.getPaddingBottom());
            cards.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
                int side = carousel.padding(r - l);
                if (cards.getPaddingStart() == side) return;
                cards.setPaddingRelative(side, cards.getPaddingTop(), side, cards.getPaddingBottom());
            });
            cards.addOnScrollListener(new RecyclerView.OnScrollListener() {
                @Override public void onScrollStateChanged(RecyclerView view, int state) {
                    // The ambient follows only once the carousel rests.
                    if (state == RecyclerView.SCROLL_STATE_IDLE && game != null) listener.onHeroShown(game);
                }
            });
            // Play stands for the middle cover: sideways moves the carousel under it.
            play.setOnKeyListener((v, keyCode, event) -> {
                if (keyCode != KeyEvent.KEYCODE_DPAD_LEFT && keyCode != KeyEvent.KEYCODE_DPAD_RIGHT) {
                    return more(keyCode, event);
                }
                if (event.getAction() == KeyEvent.ACTION_DOWN) step(keyCode == KeyEvent.KEYCODE_DPAD_RIGHT ? 1 : -1);
                return true;
            });
        }

        private boolean more(int keyCode, KeyEvent event) {
            if (!InputSource.isMoreKey(event) && keyCode != KeyEvent.KEYCODE_BUTTON_X && keyCode != KeyEvent.KEYCODE_MENU) return false;
            if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled() && game != null) listener.onGameMenu(game);
            return true;
        }

        /** Moves to the game {@code by} away, past the ends nowhere. */
        private void step(int by) {
            if (hero == null || game == null) return;
            int rtl = cards.getLayoutDirection() == View.LAYOUT_DIRECTION_RTL ? -1 : 1;
            scrollTo(hero.indexOf(game.key) + by * rtl);
        }

        private void scrollTo(int position) {
            if (hero == null || position < 0 || position >= hero.games.size()) return;
            View at = layout.findViewByPosition(position);
            if (at == null) {
                layout.scrollToPositionWithOffset(position, 0);
                return;
            }
            int shift = (at.getLeft() + at.getRight()) / 2 - cards.getWidth() / 2;
            if (Motion.reduced(context)) cards.scrollBy(shift, 0);
            else cards.smoothScrollBy(shift, 0, null, ShelfLayoutManager.CARD_MS);
        }

        /** The next frame after a finger or key brought another cover to the middle, Play says what it does for that game. */
        private void middleAt(int position) {
            if (hero == null || position < 0 || position >= hero.games.size()) return;
            // Off-centre for a frame while the padding settles: no cover has come to the middle yet.
            if (cards.getPaddingStart() != carousel.padding(cards.getWidth())) return;
            HomeModel.GameCard card = hero.games.get(position);
            write(card);
            onMiddle.accept(card);
        }

        private void write(HomeModel.GameCard card) {
            game = card;
            play.setText(HomeHero.playLabel(card));
            if (play.isFocused()) listener.onHeroFocus(play.getText());
        }

        @Override
        void show(HomeHero next, String key, boolean animate) {
            boolean first = hero == null;
            HomeModel.GameCard before = game;
            hero = next;
            int position = next.indexOf(key);
            write(next.games.get(position));
            // A game that came to the hero under a refresh brings its ambient with it.
            if (!animate && before != null && !before.key.equals(game.key)) listener.onHeroShown(game);
            carousel.forget();
            if (animate) {
                scrollTo(position);
                return;
            }
            // A refresh keeps the place; only a list that moved the game under it follows the game.
            View there = layout.findViewByPosition(position);
            if (first || there == null || Math.abs((there.getLeft() + there.getRight()) / 2 - cards.getWidth() / 2) > carousel.pitch() / 2) {
                cards.stopScroll();
                layout.scrollToPositionWithOffset(position, 0);
            }
        }
    }

    /** The hairlines between a panel's lines: from the covers' left edge to the panel's right, as a group's rows are. */
    static final class PanelLines extends RecyclerView.ItemDecoration {
        private final android.graphics.Paint line = new android.graphics.Paint();
        private final int start;

        PanelLines(Context context) {
            line.setColor(com.google.android.material.color.MaterialColors.getColor(context, R.attr.glassSeparator, 0));
            android.content.res.Resources res = context.getResources();
            line.setStrokeWidth(Math.max(1f, res.getDisplayMetrics().density * .75f));
            start = Math.round(12 * res.getDisplayMetrics().density);
        }

        @Override
        public void onDraw(android.graphics.Canvas canvas, RecyclerView parent, RecyclerView.State state) {
            boolean rtl = parent.getLayoutDirection() == View.LAYOUT_DIRECTION_RTL;
            int count = parent.getChildCount();
            for (int i = 0; i < count; i++) {
                View row = parent.getChildAt(i);
                if (parent.getChildAdapterPosition(row) <= 0) continue;
                float y = row.getTop() + row.getTranslationY();
                canvas.drawLine(rtl ? 0 : start, y, rtl ? parent.getWidth() - start : parent.getWidth(), y, line);
            }
        }
    }

    private static final class Wide extends HomeHeroViews {
        private static final int PANEL_LINES = 3;
        /** The row's 42 dp cover and 4 dp above and below it. */
        private static final int PANEL_LINE_DP = 50;
        private final View root;
        private final CoverCard cover;
        private final ImageView image;
        private final TextView imageText;
        private final TextView stateChip;
        private final TextView storeChip;
        private final TextView title;
        private final MaterialButton favourite;
        private final MaterialButton quit;
        private final View panel;
        private final RecyclerView rows;
        private final Supplier<CoverLoaders> covers;
        private final Function<String, ComputerDetails> hosts;
        private String shownKey;

        Wide(View root, ConsoleHomeAdapter.Listener listener, Supplier<CoverLoaders> covers, Function<String, ComputerDetails> hosts) {
            super(root, listener);
            this.root = root;
            this.covers = covers;
            this.hosts = hosts;
            cover = root.findViewById(R.id.hero_cover);
            image = root.findViewById(R.id.hero_cover_image);
            imageText = root.findViewById(R.id.hero_cover_text);
            stateChip = root.findViewById(R.id.hero_chip_state);
            storeChip = root.findViewById(R.id.hero_chip_store);
            title = root.findViewById(R.id.hero_title);
            favourite = root.findViewById(R.id.hero_favourite);
            quit = root.findViewById(R.id.hero_quit);
            panel = root.findViewById(R.id.hero_panel);
            rows = root.findViewById(R.id.shelf_cards);
            float density = context.getResources().getDisplayMetrics().density;
            cover.setShadow(44 * 0.75f * density, 18 * 0.75f * density);
            cover.setOnClickListener(v -> {
                if (game != null) listener.onGame(game);
            });
            cover.setOnLongClickListener(v -> {
                if (game != null) listener.onGameMenu(game);
                return game != null;
            });
            favourite.setOnClickListener(v -> {
                if (game != null) listener.onGameToggleFavourite(game);
            });
            quit.setOnClickListener(v -> {
                if (game != null) listener.onHeroQuit(game);
            });
            View.OnKeyListener more = (v, keyCode, event) -> {
                if (!InputSource.isMoreKey(event) && keyCode != KeyEvent.KEYCODE_BUTTON_X && keyCode != KeyEvent.KEYCODE_MENU) return false;
                if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled() && game != null) listener.onGameMenu(game);
                return true;
            };
            play.setOnKeyListener(more);
            cover.setOnKeyListener(more);
            favourite.setOnKeyListener(more);
            quit.setOnKeyListener(more);
            View.OnFocusChangeListener word = (v, focused) -> listener.onHeroFocus(focused ? ((TextView) v).getText() : null);
            favourite.setOnFocusChangeListener(word);
            quit.setOnFocusChangeListener(word);
            cover.setOnFocusChangeListener((v, focused) ->
                    listener.onHeroFocus(focused ? context.getString(R.string.console_hint_open) : null));
        }

        @Override
        void show(HomeHero hero, String key, boolean animate) {
            HomeModel.GameCard card = hero.games.get(hero.indexOf(key));
            boolean changed = shownKey != null && !shownKey.equals(card.key);
            game = card;
            shownKey = card.key;
            // A game that came to the hero under a refresh brings its ambient with it.
            if (changed && !animate) listener.onHeroShown(card);
            ComputerDetails host = hosts.apply(card.hostUuid);
            CoverLoaders loaders = covers.get();
            if (loaders != null) {
                loaders.bind(CoverLoaders.coverHost(host, card.source), CoverLoaders.appOf(card.title, card.appId, card.source), image, imageText);
            }
            cover.setContentDescription(ShelfAdapter.CardHolder.describe(context, card, ShelfAdapter.CardHolder.whereOf(context, card),
                    ShelfAdapter.CardHolder.stateOf(context, card)));
            title.setText(card.title);
            boolean away = OfflineLook.allAway(card.hostUuids, hosts);
            CharSequence state = away ? context.getString(R.string.console_badge_asleep) : ShelfAdapter.CardHolder.stateOf(context, card);
            if (state == null) state = ShelfAdapter.CardHolder.availabilityOf(context, card);
            stateChip.setText(state);
            stateChip.setVisibility(state == null ? View.GONE : View.VISIBLE);
            stateChip.setCompoundDrawablesRelativeWithIntrinsicBounds(!away && card.badge == HomeModel.Badge.RUNNING ? R.drawable.ic_live_dot : 0, 0, 0, 0);
            CharSequence store = card.storeName();
            storeChip.setText(store);
            storeChip.setVisibility(store == null ? View.GONE : View.VISIBLE);
            play.setText(HomeHero.playLabel(card));
            if (play.isFocused()) listener.onHeroFocus(play.getText());
            favourite.setVisibility(card.notAdded() ? View.GONE : View.VISIBLE);
            favourite.setIconResource(card.favourite ? R.drawable.ic_star_filled : R.drawable.ic_glass_star);
            favourite.setSelected(card.favourite);
            favourite.setContentDescription(context.getString(card.favourite ? R.string.console_favourite_remove : R.string.console_favourite_add));
            boolean running = !away && card.badge == HomeModel.Badge.RUNNING && host != null
                    && host.hasPermission(ComputerDetails.PERMISSION_LAUNCH);
            if (quit.isFocused() && !running) play.requestFocus();
            quit.setVisibility(running ? View.VISIBLE : View.GONE);
            int lines = rows.getAdapter() == null ? 0 : rows.getAdapter().getItemCount();
            panel.setVisibility(lines > 0 ? View.VISIBLE : View.GONE);
            // Three lines stand beside the hero, more scroll inside the panel.
            android.view.ViewGroup.LayoutParams height = rows.getLayoutParams();
            int wanted = lines > PANEL_LINES ? Math.round(PANEL_LINES * PANEL_LINE_DP * context.getResources().getDisplayMetrics().density)
                    : android.view.ViewGroup.LayoutParams.WRAP_CONTENT;
            if (height.height != wanted) {
                height.height = wanted;
                rows.setLayoutParams(height);
            }
            if (animate && changed) {
                // A panel line came to the hero: focus goes with it to Play, and the new game comes in.
                if (rows.hasFocus()) play.requestFocus();
                enter();
            }
        }

        /** The hero's parts come in together over the same short step as the TV's hero; nothing moves in the layout. */
        private void enter() {
            if (Motion.reduced(context)) return;
            for (View part : new View[] {cover, title, stateChip, storeChip}) {
                part.animate().cancel();
                part.setAlpha(0f);
                part.animate().alpha(1f).setDuration(140).start();
            }
        }
    }
}
