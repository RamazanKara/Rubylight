package com.limelight.console;

import android.content.Context;
import android.content.res.ColorStateList;
import android.view.KeyEvent;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.annotation.StringRes;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.color.MaterialColors;
import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Function;

/**
 * Home's rows as one grid: section headers, wide cards (games to continue, hosts) and library covers.
 * Cards are focusable, so the D-pad and a controller move over them like over the library grid; ids are
 * stable, so a refresh keeps the focused card focused.
 */
final class ConsoleHomeAdapter extends RecyclerView.Adapter<RecyclerView.ViewHolder> {
    interface Listener {
        void onGame(HomeModel.GameCard card);

        /** A long press (or the D-pad centre held) on a game card. */
        void onGameMenu(HomeModel.GameCard card);

        /** The Menu key on a game card: star or unstar it at once. */
        void onGameToggleFavourite(HomeModel.GameCard card);

        void onHost(HomeModel.HostCard card);
    }

    static final int HEADER = 0;
    static final int WIDE_GAME = 1;
    static final int COVER_GAME = 2;
    static final int HOST = 3;

    private static final class Row {
        final int type;
        final String key;
        @StringRes final int title;
        final HomeModel.GameCard game;
        final HomeModel.HostCard host;

        Row(int type, String key, int title, HomeModel.GameCard game, HomeModel.HostCard host) {
            this.type = type;
            this.key = key;
            this.title = title;
            this.game = game;
            this.host = host;
        }
    }

    private final Context context;
    private CoverLoaders covers;
    private final Function<String, ComputerDetails> hosts;
    private final Listener listener;
    private final List<Row> rows = new ArrayList<>();
    private final Map<String, Long> ids = new HashMap<>();
    private int coverHeightPx;

    ConsoleHomeAdapter(Context context, CoverLoaders covers, Function<String, ComputerDetails> hosts, Listener listener) {
        this.context = context;
        this.covers = covers;
        this.hosts = hosts;
        this.listener = listener;
        setHasStableIds(true);
    }

    /** The cover loaders exist once the app's client identity is known. */
    void setCovers(CoverLoaders covers) {
        this.covers = covers;
        notifyDataSetChanged();
    }

    /** Library covers are this tall (the grid's column width times 4/3). */
    void setCoverHeight(int px) {
        if (px != coverHeightPx) {
            coverHeightPx = px;
            notifyDataSetChanged();
        }
    }

    void submit(HomeModel model) {
        rows.clear();
        if (!model.favourites.isEmpty()) {
            rows.add(new Row(HEADER, "header:favourites", R.string.console_section_favourites, null, null));
            for (HomeModel.GameCard card : model.favourites) {
                rows.add(new Row(COVER_GAME, card.key, 0, card, null));
            }
        }
        if (!model.continuePlaying.isEmpty()) {
            rows.add(new Row(HEADER, "header:continue", R.string.console_section_continue, null, null));
            for (HomeModel.GameCard card : model.continuePlaying) {
                rows.add(new Row(WIDE_GAME, card.key, 0, card, null));
            }
        }
        if (!model.library.isEmpty()) {
            rows.add(new Row(HEADER, "header:library", R.string.console_section_library, null, null));
            for (HomeModel.GameCard card : model.library) {
                rows.add(new Row(COVER_GAME, card.key, 0, card, null));
            }
        }
        if (!model.hosts.isEmpty()) {
            rows.add(new Row(HEADER, "header:hosts", R.string.console_section_hosts, null, null));
            for (HomeModel.HostCard card : model.hosts) {
                rows.add(new Row(HOST, "host:" + card.uuid, 0, null, card));
            }
        }
        notifyDataSetChanged();
    }

    int viewTypeAt(int position) {
        return rows.get(position).type;
    }

    /** Position of the first card, or -1 when there is none. */
    int firstCard() {
        for (int i = 0; i < rows.size(); i++) {
            if (rows.get(i).type != HEADER) {
                return i;
            }
        }
        return -1;
    }

    /**
     * Where focus starts: the game that runs now (in Continue playing, else anywhere), otherwise the first card.
     * The Favourites row stays first on screen either way.
     */
    int initialFocus() {
        int anywhere = -1;
        for (int i = 0; i < rows.size(); i++) {
            HomeModel.GameCard game = rows.get(i).game;
            if (game != null && game.badge == HomeModel.Badge.RUNNING) {
                if (rows.get(i).type == WIDE_GAME) {
                    return i;
                }
                if (anywhere < 0) {
                    anywhere = i;
                }
            }
        }
        return anywhere >= 0 ? anywhere : firstCard();
    }

    /** The stable id of the card at the position, or null for a header or out of range. */
    String keyAt(int position) {
        if (position < 0 || position >= rows.size() || rows.get(position).type == HEADER) {
            return null;
        }
        return rows.get(position).key;
    }

    /** Position of the card with this stable id, or -1. */
    int positionOfKey(String key) {
        for (int i = 0; i < rows.size(); i++) {
            if (rows.get(i).type != HEADER && rows.get(i).key.equals(key)) {
                return i;
            }
        }
        return -1;
    }

    /** The nearest card at or before the position, else after it; -1 when there is none. */
    int nearestCard(int position) {
        for (int i = Math.min(position, rows.size() - 1); i >= 0; i--) {
            if (rows.get(i).type != HEADER) {
                return i;
            }
        }
        for (int i = Math.max(position, 0); i < rows.size(); i++) {
            if (rows.get(i).type != HEADER) {
                return i;
            }
        }
        return -1;
    }

    @Override
    public int getItemCount() {
        return rows.size();
    }

    @Override
    public int getItemViewType(int position) {
        return rows.get(position).type;
    }

    @Override
    public long getItemId(int position) {
        String key = rows.get(position).key;
        Long id = ids.get(key);
        if (id == null) {
            id = (long) ids.size() + 1;
            ids.put(key, id);
        }
        return id;
    }

    @NonNull
    @Override
    public RecyclerView.ViewHolder onCreateViewHolder(@NonNull ViewGroup parent, int type) {
        LayoutInflater inflater = LayoutInflater.from(parent.getContext());
        switch (type) {
            case HEADER:
                return new RecyclerView.ViewHolder(inflater.inflate(R.layout.console_section_header, parent, false)) { };
            case COVER_GAME:
                return new CardHolder(inflater.inflate(R.layout.console_card_cover, parent, false), true);
            default:
                return new CardHolder(inflater.inflate(R.layout.console_card_wide, parent, false), false);
        }
    }

    @Override
    public void onBindViewHolder(@NonNull RecyclerView.ViewHolder holder, int position) {
        Row row = rows.get(position);
        if (row.type == HEADER) {
            ((TextView) holder.itemView).setText(row.title);
        } else if (row.game != null) {
            ((CardHolder) holder).bindGame(row.game, row.type == COVER_GAME);
        } else {
            ((CardHolder) holder).bindHost(row.host);
        }
    }

    private final class CardHolder extends RecyclerView.ViewHolder {
        final ImageView image;
        final TextView imageText;
        final TextView title;
        final TextView subtitle;
        final TextView badge;
        final View frame;
        final boolean cover;

        CardHolder(View view, boolean cover) {
            super(view);
            this.cover = cover;
            image = view.findViewById(cover ? R.id.console_cover : R.id.console_thumb);
            imageText = view.findViewById(cover ? R.id.console_cover_text : R.id.console_thumb_text);
            frame = view.findViewById(cover ? R.id.console_cover_frame : R.id.console_thumb_frame);
            title = view.findViewById(R.id.console_title);
            subtitle = view.findViewById(R.id.console_subtitle);
            badge = view.findViewById(R.id.console_badge);
            view.setOnHoverListener((card, event) -> {
                card.setHovered(event.getActionMasked() != android.view.MotionEvent.ACTION_HOVER_EXIT);
                return false;
            });
        }

        void bindGame(HomeModel.GameCard card, boolean libraryCover) {
            if (libraryCover && coverHeightPx > 0) {
                ViewGroup.LayoutParams params = frame.getLayoutParams();
                if (params.height != coverHeightPx) {
                    params.height = coverHeightPx;
                    frame.setLayoutParams(params);
                }
            }
            image.setImageTintList(null);
            image.setScaleType(ImageView.ScaleType.CENTER_CROP);
            if (covers != null) covers.bind(hosts.apply(card.hostUuid), CoverLoaders.appOf(card.title, card.appId, card.source), image,
                    imageText);
            title.setText(card.title);
            String where = card.hostCount > 1 ? context.getString(R.string.console_card_on_hosts, card.hostCount)
                    : context.getString(R.string.console_card_on_host, card.hostName);
            subtitle.setText(where);
            CharSequence state = null;
            if (card.badge == HomeModel.Badge.RUNNING) {
                state = context.getString(R.string.console_badge_running);
            } else if (card.badge == HomeModel.Badge.SUSPENDED) {
                state = context.getString(R.string.console_badge_suspended);
            }
            badge.setText(state);
            badge.setVisibility(state == null ? View.GONE : View.VISIBLE);
            String description = state == null ? card.title + ", " + where : card.title + ", " + state + ", " + where;
            itemView.setContentDescription(card.favourite
                    ? description + ", " + context.getString(R.string.console_favourite_description) : description);
            itemView.setOnClickListener(v -> listener.onGame(card));
            itemView.setOnLongClickListener(v -> {
                listener.onGameMenu(card);
                return true;
            });
            itemView.setOnKeyListener((v, keyCode, event) -> {
                if (keyCode != KeyEvent.KEYCODE_MENU) {
                    return false;
                }
                if (event.getAction() == KeyEvent.ACTION_UP) {
                    listener.onGameToggleFavourite(card);
                }
                return true;
            });
        }

        void bindHost(HomeModel.HostCard card) {
            imageText.setVisibility(View.GONE);
            image.setImageResource(R.drawable.ic_computer);
            image.setScaleType(ImageView.ScaleType.CENTER_INSIDE);
            image.setImageTintList(ColorStateList.valueOf(
                    MaterialColors.getColor(image, com.google.android.material.R.attr.colorOnSurfaceVariant)));
            image.setVisibility(View.VISIBLE);
            title.setText(card.name);
            String line = hostLine(card);
            subtitle.setText(line);
            badge.setVisibility(View.GONE);
            itemView.setContentDescription(card.name + ", " + line);
            itemView.setOnClickListener(v -> listener.onHost(card));
            itemView.setOnLongClickListener(null);
            itemView.setLongClickable(false);
            itemView.setOnKeyListener(null);
        }

        private String hostLine(HomeModel.HostCard card) {
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
