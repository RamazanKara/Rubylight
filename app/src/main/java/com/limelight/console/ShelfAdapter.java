package com.limelight.console;

import android.content.Context;
import android.content.res.ColorStateList;
import android.os.Build;
import android.os.SystemClock;
import android.view.KeyEvent;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.ArtFit;
import com.limelight.ui.FastTravel;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Function;

/**
 * The cards of one Home shelf: games as covers (posters on TV), hosts as wide cards. Every shelf has its own adapter and
 * they all share one {@link Env} and one view pool, so a card scrolled off one shelf serves the next.
 */
final class ShelfAdapter extends RecyclerView.Adapter<ShelfAdapter.CardHolder> {
    static final int COVER_GAME = 0;
    static final int HOST = 1;

    /** One card of a shelf: a game or a host, found again across a refresh by its stable key. */
    static final class Item {
        final String key;
        final HomeModel.GameCard game;
        final HomeModel.HostCard host;

        Item(String key, HomeModel.GameCard game, HomeModel.HostCard host) {
            this.key = key;
            this.game = game;
            this.host = host;
        }

        int type() {
            return game != null ? COVER_GAME : HOST;
        }
    }

    /** What a shelf tells Home when one of its cards takes focus. */
    interface FocusListener {
        void onCardFocus(String shelf, Item item, View card);
    }

    /** What every shelf shares. */
    static final class Env {
        final Context context;
        final Function<String, ComputerDetails> hosts;
        final ConsoleHomeAdapter.Listener listener;
        /** Tells a held D-pad key from a press, so a run across the covers does not queue a focus tween per card. */
        final FastTravel travel = new FastTravel();
        CoverLoaders covers;
        int coverHeightPx;
        FocusListener focus;

        Env(Context context, CoverLoaders covers, Function<String, ComputerDetails> hosts, ConsoleHomeAdapter.Listener listener) {
            this.context = context;
            this.covers = covers;
            this.hosts = hosts;
            this.listener = listener;
        }
    }

    private final Env env;
    private String shelf = "";
    private List<Item> items = new ArrayList<>();
    private final Map<String, Long> ids = new HashMap<>();

    ShelfAdapter(Env env) {
        this.env = env;
        setHasStableIds(true);
    }

    void setItems(String shelf, List<Item> next) {
        this.shelf = shelf;
        items = next;
        notifyDataSetChanged();
    }

    Item item(int position) {
        return position >= 0 && position < items.size() ? items.get(position) : null;
    }

    int positionOfKey(String key) {
        for (int i = 0; i < items.size(); i++) {
            if (items.get(i).key.equals(key)) return i;
        }
        return -1;
    }

    @Override
    public int getItemCount() {
        return items.size();
    }

    @Override
    public int getItemViewType(int position) {
        return items.get(position).type();
    }

    @Override
    public long getItemId(int position) {
        String key = items.get(position).key;
        Long id = ids.get(key);
        if (id == null) {
            id = (long) ids.size() + 1;
            ids.put(key, id);
        }
        return id;
    }

    @NonNull
    @Override
    public CardHolder onCreateViewHolder(@NonNull ViewGroup parent, int type) {
        LayoutInflater inflater = LayoutInflater.from(parent.getContext());
        boolean cover = type == COVER_GAME;
        View view = inflater.inflate(cover ? R.layout.console_card_cover : R.layout.console_card_host, parent, false);
        int width = view.getResources().getDimensionPixelSize(cover ? R.dimen.shelf_card_width : R.dimen.shelf_host_width);
        RecyclerView.LayoutParams params = new RecyclerView.LayoutParams(width, ViewGroup.LayoutParams.WRAP_CONTENT);
        view.setLayoutParams(params);
        return new CardHolder(env, view, cover);
    }

    @Override
    public void onBindViewHolder(@NonNull CardHolder holder, int position) {
        Item item = items.get(position);
        if (item.game != null) holder.bindGame(shelf, item);
        else holder.bindHost(shelf, item);
    }

    /** The height of a cover: its card's width less the inset on both sides, times 3/2 for a TV poster, 4/3 on a phone. */
    static int coverHeight(Context context, boolean television) {
        int inset = context.getResources().getDimensionPixelSize(R.dimen.cover_inset);
        int width = context.getResources().getDimensionPixelSize(R.dimen.shelf_card_width) - 2 * inset;
        return Math.max(1, television ? width * 3 / 2 : width * 4 / 3);
    }

    static final class CardHolder extends RecyclerView.ViewHolder {
        private final Env env;
        final ImageView image;
        final TextView imageText;
        /** A TV poster has neither: the hero names the focused game. */
        final TextView title;
        final TextView subtitle;
        final TextView badge;
        final View frame;
        final View more;
        final boolean cover;
        /** The host's three dots while the D-pad has marked them; the card keeps focus. */
        private boolean moreMarked;

        CardHolder(Env env, View view, boolean cover) {
            super(view);
            this.env = env;
            this.cover = cover;
            image = view.findViewById(cover ? R.id.console_cover : R.id.console_thumb);
            imageText = view.findViewById(cover ? R.id.console_cover_text : R.id.console_thumb_text);
            frame = view.findViewById(cover ? R.id.console_cover_frame : R.id.console_thumb_frame);
            title = view.findViewById(R.id.console_title);
            subtitle = view.findViewById(R.id.console_subtitle);
            badge = view.findViewById(R.id.console_badge);
            more = view.findViewById(R.id.console_more);
            view.setOnHoverListener((card, event) -> {
                card.setHovered(event.getActionMasked() != android.view.MotionEvent.ACTION_HOVER_EXIT);
                return false;
            });
        }

        /** What a screen reader says for a game card, whether or not the card shows it. */
        static String describe(Context context, HomeModel.GameCard card, String where, CharSequence state) {
            String description = state == null ? card.title + ", " + where : card.title + ", " + state + ", " + where;
            return card.favourite ? description + ", " + context.getString(R.string.console_favourite_description) : description;
        }

        void bindGame(String shelf, Item item) {
            HomeModel.GameCard card = item.game;
            Context context = env.context;
            if (env.coverHeightPx > 0) {
                ViewGroup.LayoutParams params = frame.getLayoutParams();
                if (params.height != env.coverHeightPx) {
                    params.height = env.coverHeightPx;
                    frame.setLayoutParams(params);
                }
            }
            image.setImageTintList(null);
            // A cover is fitted to its poster by the art it is given (ArtFit).
            ArtFit.reset(image);
            if (env.covers != null) {
                env.covers.bind(env.hosts.apply(card.hostUuid), CoverLoaders.appOf(card.title, card.appId, card.source), image,
                        imageText);
            }
            String where = whereOf(context, card);
            if (title != null) title.setText(card.title);
            if (subtitle != null) subtitle.setText(where);
            CharSequence state = stateOf(context, card);
            badge.setText(state);
            badge.setVisibility(state == null ? View.GONE : View.VISIBLE);
            itemView.setContentDescription(describe(context, card, where, state));
            itemView.setOnClickListener(v -> env.listener.onGame(card));
            itemView.setOnLongClickListener(v -> {
                env.listener.onGameMenu(card);
                return true;
            });
            itemView.setOnFocusChangeListener((v, hasFocus) -> {
                // The card's own focus tween is skipped while a key is held: the new card is focused at once, the old at rest.
                if (env.travel.isActive(SystemClock.uptimeMillis())) {
                    // Focus changes before the card's drawable state does; settle the state it is moving to.
                    v.refreshDrawableState();
                    FastTravel.settle(v);
                }
                if (hasFocus && env.focus != null) env.focus.onCardFocus(shelf, item, v);
            });
            itemView.setOnKeyListener((v, keyCode, event) -> {
                if (event.getAction() == KeyEvent.ACTION_DOWN && movesFocus(keyCode)) {
                    env.travel.onMove(event.getRepeatCount(), SystemClock.uptimeMillis());
                }
                if (keyCode == KeyEvent.KEYCODE_BUTTON_X || keyCode == KeyEvent.KEYCODE_MENU) {
                    // X and Menu open the game's options (where it is starred), like a long press and like a PC's card.
                    if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) env.listener.onGameMenu(card);
                    return true;
                }
                return false;
            });
        }

        static String whereOf(Context context, HomeModel.GameCard card) {
            return card.hostCount > 1 ? context.getString(R.string.console_card_on_hosts, card.hostCount)
                    : context.getString(R.string.console_card_on_host, card.hostName);
        }

        static CharSequence stateOf(Context context, HomeModel.GameCard card) {
            if (card.badge == HomeModel.Badge.RUNNING) return context.getString(R.string.console_badge_running);
            if (card.badge == HomeModel.Badge.SUSPENDED) return context.getString(R.string.console_badge_suspended);
            return null;
        }

        private static boolean movesFocus(int keyCode) {
            return keyCode == KeyEvent.KEYCODE_DPAD_LEFT || keyCode == KeyEvent.KEYCODE_DPAD_RIGHT
                    || keyCode == KeyEvent.KEYCODE_DPAD_UP || keyCode == KeyEvent.KEYCODE_DPAD_DOWN;
        }

        void bindHost(String shelf, Item item) {
            HomeModel.HostCard card = item.host;
            Context context = env.context;
            imageText.setVisibility(View.GONE);
            image.setImageResource(R.drawable.ic_computer);
            image.setScaleType(ImageView.ScaleType.CENTER_INSIDE);
            image.setImageTintList(ColorStateList.valueOf(
                    MaterialColors.getColor(image, com.google.android.material.R.attr.colorOnSurfaceVariant)));
            image.setVisibility(View.VISIBLE);
            title.setText(card.name);
            String line = hostLine(context, card);
            subtitle.setText(line);
            badge.setVisibility(View.GONE);
            itemView.setContentDescription(card.name + ", " + line);
            itemView.setOnClickListener(v -> env.listener.onHost(card));
            itemView.setOnLongClickListener(v -> {
                env.listener.onHostOptions(card);
                return true;
            });
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                // A mouse's right button.
                itemView.setOnContextClickListener(v -> {
                    env.listener.onHostOptions(card);
                    return true;
                });
            }
            // The dots are marked by the D-pad, never a focus stop of their own (a style can make buttons focusable).
            more.setFocusable(false);
            more.setContentDescription(context.getString(R.string.library_more_for, card.name));
            more.setOnClickListener(v -> env.listener.onHostOptions(card));
            markMore(false);
            itemView.setOnFocusChangeListener((v, hasFocus) -> {
                if (!hasFocus) markMore(false);
                else if (env.focus != null) env.focus.onCardFocus(shelf, item, v);
            });
            itemView.setOnKeyListener((v, keyCode, event) -> {
                if (event.getAction() == KeyEvent.ACTION_DOWN && movesFocus(keyCode)) {
                    env.travel.onMove(event.getRepeatCount(), SystemClock.uptimeMillis());
                }
                return onHostKey(card, keyCode, event);
            });
        }

        private void markMore(boolean marked) {
            moreMarked = marked;
            more.setHovered(marked);
        }

        /**
         * Menu, X and Shift+F10 open the options at once. Toward the end of the row with nothing further
         * that way, the D-pad marks the three dots (the card keeps focus, as in the list of PCs); Center
         * opens them, the way back unmarks.
         */
        private boolean onHostKey(HomeModel.HostCard card, int keyCode, KeyEvent event) {
            boolean down = event.getAction() == KeyEvent.ACTION_DOWN;
            if (keyCode == KeyEvent.KEYCODE_MENU || keyCode == KeyEvent.KEYCODE_BUTTON_X
                    || (keyCode == KeyEvent.KEYCODE_F10 && event.isShiftPressed())) {
                markMore(false);
                if (!down && !event.isCanceled()) {
                    env.listener.onHostOptions(card);
                }
                return true;
            }
            if (!event.hasNoModifiers()) {
                return false;
            }
            boolean rtl = itemView.getLayoutDirection() == View.LAYOUT_DIRECTION_RTL;
            int towardMore = rtl ? KeyEvent.KEYCODE_DPAD_LEFT : KeyEvent.KEYCODE_DPAD_RIGHT;
            int towardCard = rtl ? KeyEvent.KEYCODE_DPAD_RIGHT : KeyEvent.KEYCODE_DPAD_LEFT;
            if (!moreMarked) {
                if (keyCode == towardMore && atRowEnd()) {
                    if (down) markMore(true);
                    return true;
                }
                return false;
            }
            if (keyCode == towardMore) {
                // The press that marked the dots, and the ones after it, stay on them.
                return true;
            }
            if (keyCode == towardCard) {
                if (down) markMore(false);
                return true;
            }
            if (keyCode == KeyEvent.KEYCODE_DPAD_CENTER || keyCode == KeyEvent.KEYCODE_ENTER
                    || keyCode == KeyEvent.KEYCODE_NUMPAD_ENTER || keyCode == KeyEvent.KEYCODE_SPACE
                    || keyCode == KeyEvent.KEYCODE_BUTTON_A) {
                if (!down && !event.isCanceled()) {
                    markMore(false);
                    more.performClick();
                }
                return true;
            }
            if (keyCode == KeyEvent.KEYCODE_BACK || keyCode == KeyEvent.KEYCODE_BUTTON_B
                    || keyCode == KeyEvent.KEYCODE_ESCAPE) {
                if (!down && !event.isCanceled()) markMore(false);
                return true;
            }
            markMore(false);
            return false;
        }

        /** True when no card follows this one on its shelf, so the end direction has nowhere else to go. */
        private boolean atRowEnd() {
            int position = getBindingAdapterPosition();
            if (position == RecyclerView.NO_POSITION) {
                return false;
            }
            RecyclerView.Adapter<?> adapter = getBindingAdapter();
            return adapter == null || position >= adapter.getItemCount() - 1;
        }

        static String hostLine(Context context, HomeModel.HostCard card) {
            switch (card.line) {
                case GAMES:
                    return context.getString(R.string.console_host_games, card.gameCount);
                case SAVED_GAMES:
                    return context.getString(R.string.console_host_saved_games, card.gameCount);
                case NO_LIBRARY:
                    return context.getString(R.string.console_host_no_library);
                default:
                    return context.getString(R.string.console_host_offline);
            }
        }
    }
}
