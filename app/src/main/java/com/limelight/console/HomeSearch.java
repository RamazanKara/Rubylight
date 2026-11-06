package com.limelight.console;

import android.text.Editable;
import android.text.TextWatcher;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewTreeObserver;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;

import androidx.activity.ComponentActivity;
import androidx.activity.OnBackPressedCallback;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.limelight.R;
import com.limelight.ui.Motion;

import java.text.Normalizer;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;
import java.util.function.Supplier;

/**
 * Search on Home's own page: the field takes the top row's place, the covers narrow as you type and Back goes back to where
 * the search was opened from. Nothing leaves the screen, so the ambient and the place on Home stay.
 */
final class HomeSearch {
    private static final int FADE_MS = 150;
    private static final String SHELF = "search";

    private final ComponentActivity activity;
    private final ShelfAdapter.Env env;
    private final ShelfAdapter adapter;
    private final Supplier<HomeModel> model;
    private final View bar;
    private final EditText field;
    private final View clear;
    private final RecyclerView results;
    private final View none;
    private final View list;
    private final View topRow;
    private final View bottomRow;
    private final View footFade;
    private final Runnable onOpen;
    private final Runnable onClose;
    private final GridLayoutManager grid;
    private final OnBackPressedCallback back = new OnBackPressedCallback(false) {
        @Override public void handleOnBackPressed() {
            close();
        }
    };
    private final ViewTreeObserver.OnGlobalLayoutListener keyboard = this::fitKeyboard;
    private View opener;
    private int restBottom;

    /** Null on a layout without the field (a TV's). */
    static HomeSearch of(ComponentActivity activity, ShelfAdapter.Env env, Supplier<HomeModel> model, Runnable onOpen, Runnable onClose) {
        return activity.findViewById(R.id.home_search_bar) == null ? null : new HomeSearch(activity, env, model, onOpen, onClose);
    }

    private HomeSearch(ComponentActivity activity, ShelfAdapter.Env env, Supplier<HomeModel> model, Runnable onOpen, Runnable onClose) {
        this.activity = activity;
        this.env = env;
        this.model = model;
        this.onOpen = onOpen;
        this.onClose = onClose;
        bar = activity.findViewById(R.id.home_search_bar);
        field = activity.findViewById(R.id.home_search_field);
        clear = activity.findViewById(R.id.home_search_clear);
        results = activity.findViewById(R.id.home_search_results);
        none = activity.findViewById(R.id.home_search_none);
        list = activity.findViewById(R.id.console_list);
        topRow = activity.findViewById(R.id.home_top_row);
        bottomRow = activity.findViewById(R.id.home_bottom_row);
        footFade = activity.findViewById(R.id.home_foot_fade);
        adapter = new ShelfAdapter(env);
        grid = new GridLayoutManager(activity, 2);
        results.setLayoutManager(grid);
        results.setItemAnimator(null);
        results.setAdapter(adapter);
        activity.getOnBackPressedDispatcher().addCallback(activity, back);
        field.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence s, int start, int count, int after) { }
            @Override public void onTextChanged(CharSequence s, int start, int before, int count) { }
            @Override public void afterTextChanged(Editable text) {
                if (isOpen()) showMatches();
            }
        });
        field.setOnEditorActionListener((v, action, event) -> {
            if (action != EditorInfo.IME_ACTION_SEARCH) return false;
            keyboard(false);
            focusFirstResult();
            return true;
        });
        field.setOnKeyListener((v, keyCode, event) -> {
            if (keyCode != KeyEvent.KEYCODE_DPAD_DOWN || event.getAction() != KeyEvent.ACTION_DOWN) return false;
            keyboard(false);
            return focusFirstResult();
        });
        clear.setOnClickListener(v -> {
            field.setText("");
            field.requestFocus();
        });
    }

    boolean isOpen() {
        return bar.getVisibility() == View.VISIBLE;
    }

    /** The field takes the top row's place; {@code from} gets focus back at the end. */
    void open(View from) {
        if (isOpen()) return;
        opener = from;
        fitColumns();
        field.setText("");
        results.scrollToPosition(0);
        topRow.setVisibility(View.INVISIBLE);
        if (bottomRow != null) bottomRow.setVisibility(View.GONE);
        if (footFade != null) footFade.setVisibility(View.GONE);
        list.setVisibility(View.INVISIBLE);
        bar.setVisibility(View.VISIBLE);
        results.setVisibility(View.VISIBLE);
        back.setEnabled(true);
        showMatches();
        comeIn(bar);
        comeIn(results);
        field.requestFocus();
        keyboard(true);
        results.getViewTreeObserver().addOnGlobalLayoutListener(keyboard);
        onOpen.run();
    }

    /** Back at Home where it was; focus is on what opened the search. */
    void close() {
        if (!isOpen()) return;
        keyboard(false);
        results.getViewTreeObserver().removeOnGlobalLayoutListener(keyboard);
        back.setEnabled(false);
        bar.setVisibility(View.GONE);
        results.setVisibility(View.GONE);
        none.setVisibility(View.GONE);
        topRow.setVisibility(View.VISIBLE);
        list.setVisibility(View.VISIBLE);
        onClose.run();
        if (opener != null && !opener.isInTouchMode()) opener.requestFocus();
        opener = null;
    }

    /** The library changed while the search is open: the same search over the new games. */
    void onModel() {
        if (isOpen()) showMatches();
    }

    /** Padding as Home's list has it: the field's place at the top, the safe area at the foot. */
    void setPadding(int left, int top, int right, int bottom) {
        restBottom = bottom;
        results.setPadding(left, top, right, bottom);
    }

    private void showMatches() {
        String query = field.getText().toString();
        clear.setVisibility(query.isEmpty() ? View.GONE : View.VISIBLE);
        List<ShelfAdapter.Item> items = new ArrayList<>();
        for (HomeModel.GameCard card : matches(model.get().cardsUnder(HomeModel.FILTER_ALL), query)) {
            items.add(new ShelfAdapter.Item(card.key, card, null));
        }
        adapter.setItems(SHELF, items);
        none.setVisibility(items.isEmpty() ? View.VISIBLE : View.GONE);
    }

    private void fitColumns() {
        android.content.res.Resources res = activity.getResources();
        int usual = res.getDimensionPixelSize(com.limelight.ui.GlassForm.of(activity) == com.limelight.ui.GlassForm.HANDHELD
                ? R.dimen.shelf_card_width_handheld : R.dimen.shelf_card_width);
        int width = list.getWidth() - results.getPaddingLeft() - results.getPaddingRight();
        int columns = Math.max(2, width / Math.max(1, usual));
        int slot = width / columns;
        grid.setSpanCount(columns);
        env.cardWidthPx = slot;
        env.coverHeightPx = ShelfAdapter.coverHeight(activity, false, slot);
    }

    private boolean focusFirstResult() {
        RecyclerView.ViewHolder first = results.findViewHolderForAdapterPosition(0);
        return first != null && first.itemView.requestFocus();
    }

    private void keyboard(boolean show) {
        InputMethodManager manager = (InputMethodManager) activity.getSystemService(android.content.Context.INPUT_METHOD_SERVICE);
        if (manager == null) return;
        if (show) manager.showSoftInput(field, InputMethodManager.SHOW_IMPLICIT);
        else manager.hideSoftInputFromWindow(field.getWindowToken(), 0);
    }

    /** The last covers stay reachable above the keyboard. */
    private void fitKeyboard() {
        WindowInsetsCompat insets = ViewCompat.getRootWindowInsets(results);
        int ime = insets == null ? 0 : insets.getInsets(WindowInsetsCompat.Type.ime()).bottom;
        int bottom = Math.max(restBottom, ime);
        if (results.getPaddingBottom() != bottom) results.setPadding(results.getPaddingLeft(), results.getPaddingTop(), results.getPaddingRight(), bottom);
    }

    private void comeIn(View view) {
        view.animate().cancel();
        if (Motion.reduced(activity)) {
            view.setAlpha(1f);
            return;
        }
        view.setAlpha(0f);
        view.animate().alpha(1f).setDuration(FADE_MS).start();
    }

    /**
     * The games whose title has every word of the search, ignoring case and accents: titles that start with it first, then
     * those with a word that starts with it, each alphabetically. An empty search has them all.
     */
    static List<HomeModel.GameCard> matches(List<HomeModel.GameCard> games, String query) {
        String wanted = folded(query.trim());
        String[] words = wanted.isEmpty() ? new String[0] : wanted.split("\\s+");
        List<HomeModel.GameCard> found = new ArrayList<>();
        for (HomeModel.GameCard card : games) {
            String title = folded(card.title);
            boolean all = true;
            for (String word : words) all &= title.contains(word);
            if (all) found.add(card);
        }
        Collections.sort(found, (a, b) -> {
            int rank = rank(a.title, wanted) - rank(b.title, wanted);
            return rank != 0 ? rank : folded(a.title).compareTo(folded(b.title));
        });
        return found;
    }

    private static int rank(String title, String wanted) {
        String folded = folded(title);
        if (wanted.isEmpty() || folded.startsWith(wanted)) return 0;
        return folded.contains(" " + wanted) ? 1 : 2;
    }

    /** Lower case without accents, so "pokemon" finds "Pokémon". */
    static String folded(String text) {
        return Normalizer.normalize(text, Normalizer.Form.NFD).replaceAll("\\p{M}+", "").toLowerCase(Locale.ROOT);
    }
}
