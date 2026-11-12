package com.limelight.console;

import android.content.Intent;
import android.content.res.Configuration;
import android.graphics.drawable.ColorDrawable;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.preference.PreferenceManager;
import android.view.View;
import android.view.ViewGroup;

import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.LinearLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.limelight.R;
import com.limelight.computers.ComputerManagerService;
import com.limelight.computers.HostActions;
import com.limelight.computers.PairingWatch;
import com.limelight.computers.HostMenu;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.ActionSheet;
import com.limelight.ui.AppTheme;
import com.limelight.ui.BackdropView;
import com.limelight.ui.InputSource;
import com.limelight.ui.Motion;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.UiHelper;

import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Home: the games to continue, the library across the paired hosts and the hosts. It paints what is saved
 * first and refreshes from the hosts that answer; a host without the native API shows its classic app list
 * and opens the classic library when tapped. The back button leaves the app.
 */
public class ConsoleHomeActivity extends NightjarActivity implements HostFeed.Listener {
    /** How long after coming to the front "no hosts" waits for the first host to answer. */
    private static final long SETTLE_MS = 1500;
    private static final long DEBOUNCE_MS = 250;
    /** How long Home waits for saved PCs to answer before it shows them offline: a poll that gets no answer is done by then. */
    private static final long WAIT_LIMIT_MS = 10_000;
    private static final int COVER_WIDTH_DP = 140;

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor(r -> {
        Thread thread = new Thread(r, "console-home");
        thread.setDaemon(true);
        return thread;
    });
    /** Stars have their own thread, so a toggle does not wait behind a refresh from the hosts. */
    private final ExecutorService starWorker = Executors.newSingleThreadExecutor(r -> {
        Thread thread = new Thread(r, "console-home-star");
        thread.setDaemon(true);
        return thread;
    });
    private HostFeed feed;
    private PairingWatch pairingWatch;
    private android.widget.Button pairingBanner;
    private List<PairingWatch.Waiting> waitingToPair = java.util.Collections.emptyList();
    /** The phone's Home: one grid of headers and cards. Null on TV. */
    private ConsoleHomeListAdapter adapter;
    private GridLayoutManager grid;
    /** The TV's Home: shelves under a hero. Null on a phone. */
    private ConsoleHomeAdapter shelves;
    private CoverLoaders covers;
    private RecyclerView list;
    private HomeAmbience ambience;
    private HomeChrome chrome;
    private HomeHints hints;
    private HostCapsule capsule;
    private HomeSearch search;
    private ShelfAdapter.Env searchEnv;
    private View loading;
    private View empty;
    private volatile ConsoleRepository repository;
    private int generation;
    private String refreshedSignature;
    /** What the last reload drew the PCs from; a poll that brings nothing else does not reload. */
    private final HostsDrawn drawn = new HostsDrawn();
    private boolean settled;
    private boolean shown;
    private boolean focusPending = true;
    /**
     * Until the first key or touch Home is at its start, whatever arrives. The hosts' answer comes right after what was saved and
     * can change the shelves: the start follows it, from the top of the list.
     */
    private boolean atStart = true;
    private int columns = 2;
    private int lastWidthPx;

    private final HostActions hostActions = new HostActions(this, new HostActions.Callbacks() {
        @Override public ComputerManagerService.ComputerManagerBinder binder() { return feed == null ? null : feed.binder(); }
        @Override public boolean isForeground() { return shown; }
        @Override public void onPaired(ComputerDetails computer) { }
        @Override public void onRemoved(ComputerDetails computer) {
            feed.forget(computer.uuid);
            refreshedSignature = null;
            scheduleReload();
        }
        @Override public void onRenamed(ComputerDetails computer) {
            refreshedSignature = null;
            scheduleReload();
        }
        @Override public void onWakeSent(ComputerDetails computer) { }
        // A form change waited for the wake; asked once the wake's end is done, as on the PCs page.
        @Override public void onWakeEnded(ComputerDetails computer) { followFormSoon(); }
        @Override public void pausePolling() { feed.pause(); }
        @Override public void resumePolling() { if (shown) feed.resume(); }
        @Override public void pairingChanged() {
            pairingWatch.refreshSoon();
            if (shown) pairingWatch.poll(feed.hosts());
        }
    });
    private final HomeWake wake = new HomeWake(this, handler, uuid -> feed == null ? null : feed.host(uuid),
            this::sendWake, () -> {
        refreshedSignature = null;
        scheduleReload();
        // A form change waited for the wake.
        followForm();
    });
    /** A "which PC?" question is pending; further taps on games wait for its answer. */
    private boolean choosing;
    private ActionSheet viewSheet;
    /** Grid or rows, by store or not, the card size: the stored choice, else this device's look. */
    private HomeView view;
    private HomeModel model = HomeModel.empty();

    private final Runnable reload = this::reloadNow;
    private final Runnable settle = () -> {
        settled = true;
        reloadNow();
    };
    private final Runnable stopWaiting = () -> feed.stopWaiting();

    /** The data behind the screen; the debug demo overrides this with canned hosts. */
    protected ConsoleRepository createRepository(String uniqueId) {
        return ConsoleRepository.create(this, uniqueId);
    }

    /** Its chrome follows the form: a rotation into another form lays it out again. */
    @Override
    protected boolean followsForm() {
        return true;
    }

    /** A pairing, a wake or a game being added keeps this screen until it is done; it then follows the form. */
    @Override
    protected boolean holdsForm() {
        return hostActions.isPairing() || hostActions.isWaking() || hostActions.isShowingError() || wake.isWaiting()
                || !adding.isEmpty();
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        setContentView(R.layout.activity_console_home);
        UiHelper.notifyNewRootView(this);

        TopAppBar.configure(this, R.id.console_toolbar, getString(R.string.console_title), 0);
        ShellBar.install(this, ShellBar.Destination.HOME);
        ShellBar.installBack(this, ShellBar.Destination.HOME);

        loading = findViewById(R.id.console_loading);
        empty = findViewById(R.id.console_empty);
        findViewById(R.id.console_empty_hosts).setOnClickListener(v -> openHosts());

        list = findViewById(R.id.console_list);
        // A RecyclerView makes itself focusable in touch mode whatever its layout says. A finger that ends key navigation then
        // hands the keys' focus to the list, which gives it to the first shelf strip below Play; that shelf's snap starts a
        // scroll, and the list takes the finger's down to stop it, so the tap never reached Play. Keys still focus the list.
        list.setFocusableInTouchMode(false);
        feed = new HostFeed(this, this);
        pairingBanner = findViewById(R.id.home_pairing_banner);
        pairingBanner.setOnClickListener(v -> hostActions.approvePairing(waitingToPair));
        pairingWatch = new PairingWatch(hostActions.pairingReader(), HostMenu::mayApprovePairing,
                task -> new Thread(task, "Devices waiting to pair").start(), handler::post, this::showWaitingToPair);
        ConsoleHomeAdapter.Listener listener = new ConsoleHomeAdapter.Listener() {
            @Override
            public void onGame(HomeModel.GameCard card) {
                openGame(card);
            }

            @Override
            public void onGameMenu(HomeModel.GameCard card) {
                askFavourite(card);
            }

            @Override
            public void onGameToggleFavourite(HomeModel.GameCard card) {
                setFavourite(card, !card.favourite);
            }

            @Override
            public void onHost(HomeModel.HostCard card) {
                openHost(card);
            }

            @Override
            public void onHostOptions(HomeModel.HostCard card) {
                capsule.showOptions(card);
            }

            @Override
            public void onAllGames(String filter) {
                startActivity(new Intent(ConsoleHomeActivity.this, allGamesActivity())
                        .putExtra(AllGamesActivity.EXTRA_FILTER, filter));
            }

            @Override
            public void onHeroPlay(HomeModel.GameCard card) {
                playHero(card);
            }

            @Override
            public void onHeroQuit(HomeModel.GameCard card) {
                quitHero(card);
            }

            @Override
            public void onHeroShown(HomeModel.GameCard card) {
                if (ambience != null) ambience.refresh(new ShelfAdapter.Item(card.key, card, null));
            }

            @Override
            public void onHeroFocus(CharSequence go) {
                if (go != null && hints != null) hints.onFocus(go, getString(R.string.console_hint_options));
            }
        };
        view = HomeView.read(PreferenceManager.getDefaultSharedPreferences(this), isTelevision());
        if (view.layout == HomeView.Layout.ROWS) {
            setUpShelves(listener);
        } else {
            setUpGrid(listener);
        }
        chrome = new HomeChrome(this, list, view.layout == HomeView.Layout.ROWS);
        capsule = new HostCapsule(this, findViewById(R.id.glass_host), findViewById(R.id.home_pc), new HostCapsule.Screen() {
            @Override public HomeModel model() { return model; }
            @Override public ComputerDetails host(String uuid) { return feed.host(uuid); }
            @Override public ComputerManagerService.ComputerManagerBinder binder() { return feed.binder(); }
            @Override public HostActions actions() { return hostActions; }
        });
        setUpAmbience();
        setUpViewButton();
        setUpSearch(listener);
        feed.bind();
    }

    /** Search on the page where the layout has the field; a TV's opens All games. */
    private void setUpSearch(ConsoleHomeAdapter.Listener listener) {
        searchEnv = new ShelfAdapter.Env(this, covers, feed::host, listener);
        search = HomeSearch.of(this, searchEnv, () -> model, () -> {
            if (hints != null) hints.onFocus(null, null);
        }, () -> {
            if (chrome.form() == com.limelight.ui.GlassForm.PHONE) {
                findViewById(R.id.home_bottom_row).setVisibility(View.VISIBLE);
                findViewById(R.id.home_foot_fade).setVisibility(View.VISIBLE);
            }
        });
        chrome.setSearch(search);
        View.OnClickListener open = v -> {
            if (search != null) search.open(v);
            else startActivity(new Intent(this, allGamesActivity()));
        };
        for (int id : new int[] {R.id.home_search, R.id.home_search_top}) {
            View button = findViewById(id);
            if (button != null) button.setOnClickListener(open);
        }
    }

    /** By the window's type; every layout has the backdrop now. */
    private boolean isTelevision() {
        return com.limelight.ui.GlassForm.of(this) == com.limelight.ui.GlassForm.TELEVISION;
    }

    /**
     * View, at the end of the shell bar: grid or rows, by store, by type or not, and the card size. The choice applies once the
     * sheet closes, so trying several costs one rebuild.
     */
    private void setUpViewButton() {
        View button = findViewById(R.id.console_view);
        button.setOnClickListener(v -> showViewSheet());
        com.limelight.ui.GlassForm form = com.limelight.ui.GlassForm.of(this);
        if (form != com.limelight.ui.GlassForm.PHONE) {
            // A TV's layout has one search; the other forms' top row shows its own and hides the foot's.
            View top = findViewById(R.id.home_search_top);
            ShellBar.linkTopRow(this, R.id.home_pc, R.id.console_view,
                    top != null && top.getVisibility() == View.VISIBLE ? R.id.home_search_top : R.id.home_search);
            return;
        }
        // A phone's bar is at the foot: View is past its last tab.
        ViewGroup tabs = findViewById(R.id.shell_bar_items);
        if (tabs != null && tabs.getChildCount() > 0) {
            View last = tabs.getChildAt(tabs.getChildCount() - 1);
            last.setNextFocusRightId(R.id.console_view);
            button.setNextFocusLeftId(last.getId());
        }
        button.setNextFocusRightId(R.id.console_view);
    }

    /** For tests and the demo's captures: the view Home shows. */
    HomeView view() {
        return view;
    }

    private void showViewSheet() {
        if (viewSheet != null) {
            return;
        }
        final HomeView before = view;
        // Off a phone View sits in the top row: its panel hangs under it, as the PC sheet hangs under the capsule.
        ActionSheet sheet = com.limelight.ui.GlassForm.of(this) == com.limelight.ui.GlassForm.PHONE
                ? new ActionSheet(this, getString(R.string.home_view))
                : ActionSheet.under(this, getString(R.string.home_view), findViewById(R.id.console_view));
        viewSheet = sheet;
        sheet.addSection(R.string.home_view_layout);
        // A list's lines have one size, so Size shows only for the covers' layouts.
        View[] sizeSection = new View[2];
        addChoices(sheet, new int[] {R.drawable.ic_library, R.drawable.ic_view_list, R.drawable.ic_view_rows},
                new int[] {R.string.home_view_layout_grid, R.string.home_view_layout_list, R.string.home_view_layout_rows},
                view.layout.ordinal(), at -> {
                    view = view.with(HomeView.Layout.values()[at]);
                    for (View part : sizeSection) part.setVisibility(view.layout == HomeView.Layout.LIST ? View.GONE : View.VISIBLE);
                });
        sheet.addSection(R.string.home_view_group);
        HomeView.Group[] groups = HomeView.offeredGroups(model.hasCollections());
        int[] groupIcons = new int[groups.length];
        int[] groupLabels = new int[groups.length];
        for (int i = 0; i < groups.length; i++) {
            groupIcons[i] = groups[i] == HomeView.Group.STORE ? R.drawable.ic_view_group_store
                    : groups[i] == HomeView.Group.TYPE ? R.drawable.ic_view_group_type
                    : groups[i] == HomeView.Group.COLLECTION ? R.drawable.ic_folder : R.drawable.ic_view_group_none;
            groupLabels[i] = groups[i] == HomeView.Group.STORE ? R.string.home_view_group_store
                    : groups[i] == HomeView.Group.TYPE ? R.string.home_view_group_type
                    : groups[i] == HomeView.Group.COLLECTION ? R.string.home_view_group_collection : R.string.home_view_group_none;
        }
        // Collection chosen while no PC names any shows by store, and says so.
        addChoices(sheet, groupIcons, groupLabels, Math.max(0, java.util.Arrays.asList(groups).indexOf(view.group)),
                at -> view = view.with(groups[at]));
        sizeSection[0] = sheet.addSection(R.string.home_view_size);
        sizeSection[1] = addChoices(sheet, new int[] {R.drawable.ic_view_size_small, R.drawable.ic_view_size_medium, R.drawable.ic_view_size_large},
                new int[] {R.string.home_view_size_small, R.string.home_view_size_medium, R.string.home_view_size_large},
                view.size.ordinal(), at -> view = view.with(HomeView.Size.values()[at]));
        for (View part : sizeSection) part.setVisibility(view.layout == HomeView.Layout.LIST ? View.GONE : View.VISIBLE);
        sheet.setOnClosed(() -> {
            viewSheet = null;
            if (!view.equals(before) && !isFinishing() && !isDestroyed()) {
                view.save(PreferenceManager.getDefaultSharedPreferences(this));
                recreate();
            }
        });
        sheet.show();
    }

    /** One choice of the View sheet: a row of toggles of which exactly one is on; the row is returned. */
    private static android.widget.LinearLayout addChoices(ActionSheet sheet, int[] icons, int[] labels, int chosen, java.util.function.IntConsumer choose) {
        android.widget.LinearLayout row = sheet.addToggleRow();
        com.google.android.material.button.MaterialButton[] toggles =
                new com.google.android.material.button.MaterialButton[labels.length];
        for (int i = 0; i < labels.length; i++) {
            final int at = i;
            toggles[i] = sheet.addToggle(row, icons[i], sheet.getContext().getString(labels[i]), i == chosen);
            toggles[i].setOnClickListener(v -> {
                for (int j = 0; j < toggles.length; j++) toggles[j].setChecked(j == at);
                choose.accept(at);
            });
        }
        return row;
    }

    private void setUpGrid(ConsoleHomeAdapter.Listener listener) {
        grid = new HomeGridLayoutManager(this, columns, position -> {
            int first = adapter.firstCard();
            return first >= 0 && first < position;
        });
        grid.setSpanSizeLookup(new GridLayoutManager.SpanSizeLookup() {
            @Override
            public int getSpanSize(int position) {
                return spanOf(adapter.viewTypeAt(position), columns);
            }
        });
        list.setLayoutManager(grid);
        list.setItemAnimator(null);
        list.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            fitColumns(r - l);
            ConsoleHomeListAdapter.fadeHeaders(list);
        });
        list.addOnScrollListener(new RecyclerView.OnScrollListener() {
            @Override
            public void onScrolled(RecyclerView view, int dx, int dy) {
                ConsoleHomeListAdapter.fadeHeaders(view);
            }
        });
        adapter = new ConsoleHomeListAdapter(this, null, feed::host, listener, isTelevision());
        adapter.setListLayout(view.layout == HomeView.Layout.LIST);
        list.setAdapter(adapter);
    }

    /** Headers take the whole row; wide cards take two columns, or the whole row when it has at most three. */
    static int spanOf(int viewType, int columns) {
        switch (viewType) {
            case ConsoleHomeListAdapter.HEADER:
            case ConsoleHomeListAdapter.LIST_GAME:
                return columns;
            case ConsoleHomeListAdapter.COVER_GAME:
                return 1;
            default:
                return columns <= 3 ? columns : 2;
        }
    }

    /** Columns for a list this wide: about 130 dp each, and an even number from four on so wide cards pair up. */
    static int columnsFor(int widthDp) {
        return columnsFor(widthDp, HomeView.Size.MEDIUM);
    }

    /** The same for cards of the chosen size: about 130 dp each at the medium size. */
    static int columnsFor(int widthDp, HomeView.Size size) {
        int columns = Math.max(2, widthDp / Math.round(130 * size.scale));
        return columns >= 4 && columns % 2 == 1 ? columns - 1 : columns;
    }

    /** TV posters across a grid this wide: as many as fit at the shelves' width times the size, at least two. */
    static int tvColumnsFor(int contentPx, int cardPx) {
        return Math.max(2, contentPx / Math.max(1, cardPx));
    }

    private void fitColumns(int widthPx) {
        if (widthPx <= 0 || widthPx == lastWidthPx) {
            return;
        }
        lastWidthPx = widthPx;
        int content = widthPx - list.getPaddingLeft() - list.getPaddingRight();
        if (isTelevision()) {
            int card = Math.round(getResources().getDimensionPixelSize(R.dimen.shelf_card_width) * view.size.scale);
            int count = tvColumnsFor(content, card);
            // Each poster fills its column, so the grid spans the screen.
            int columnWidth = content / count;
            list.post(() -> {
                if (count != columns) {
                    columns = count;
                    grid.setSpanCount(count);
                }
                adapter.setCardWidth(columnWidth);
                adapter.setCoverHeight(ShelfAdapter.coverHeight(this, true, columnWidth));
            });
            return;
        }
        float density = getResources().getDisplayMetrics().density;
        int count = columnsFor(Math.round(widthPx / density), view.size);
        // A card keeps its margin around it.
        int margin = getResources().getDimensionPixelSize(R.dimen.cover_inset);
        int columnWidth = content / count - 2 * margin;
        list.post(() -> {
            if (count != columns) {
                columns = count;
                grid.setSpanCount(count);
            }
            adapter.setCoverHeight(Math.max(1, columnWidth * 4 / 3));
        });
    }

    /** The shelves: a vertical list of rows that snaps the focused row to the top and returns to each row's last card. */
    private void setUpShelves(ConsoleHomeAdapter.Listener listener) {
        shelves = new ConsoleHomeAdapter(this, null, feed::host, listener);
        ShelfLayoutManager rows = new ShelfLayoutManager(this, LinearLayoutManager.VERTICAL,
                () -> shelves.isFastTravel() || Motion.reduced(this));
        rows.setRowFocus(shelves);
        shelves.setAboveHero(from -> ShellBar.nearestInTopRow(this, from, R.id.home_pc, R.id.console_view, R.id.home_search_top));
        list.setLayoutManager(rows);
        list.setItemAnimator(null);
        list.setItemViewCacheSize(2);
        shelves.setGroup(view.group);
        shelves.setCardScale(view.size.scale);
        shelves.setTelevision(isTelevision());
        shelves.setHeroes(!isTelevision());
        list.setAdapter(shelves);
    }

    private void setUpAmbience() {
        BackdropView backdrop = findViewById(R.id.home_backdrop);
        View hero = findViewById(R.id.home_hero);
        // It fades at each change of game; its lines do not overlap, so that needs no offscreen layer.
        if (hero != null && android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.N) hero.forceHasOverlappingRendering(false);
        int canvas = isTelevision() ? AppTheme.tvCanvas(this)
                : com.google.android.material.color.MaterialColors.getColor(backdrop, R.attr.glassCanvas);
        chrome.setUpBackdrop(backdrop, canvas);
        // The window shows the canvas until the first backdrop covers it, then stops painting under it.
        getWindow().setBackgroundDrawable(new ColorDrawable(canvas));
        // The TV's safe-area padding sits on the content view, which would otherwise clip the backdrop that reaches past it.
        ((ViewGroup) findViewById(android.R.id.content)).setClipChildren(false);
        backdrop.setCanvasColor(canvas);
        backdrop.setDim(AppTheme.isHighContrast(this) ? 0.85f : 1f);
        backdrop.setOnCovered(() -> getWindow().setBackgroundDrawable(null));
        java.util.function.BooleanSupplier fast = shelves != null ? shelves::isFastTravel : adapter::isFastTravel;
        ambience = new HomeAmbience(this, backdrop, hero, feed::host, () -> covers, fast);
        InputSource start = InputSource.last() != null ? InputSource.last() : isTelevision()
                ? (com.limelight.ui.StreamMenuHint.hasGamepad() ? InputSource.PAD : InputSource.REMOTE) : InputSource.NONE;
        ambience.setInput(start, 0);
        hints = new HomeHints(this, findViewById(R.id.home_hints), start);
        ShelfAdapter.FocusListener focus = (shelf, item, card) -> {
            hints.onFocus(HomeAmbience.goWord(this, item.host == null), item.all != null ? null : getString(R.string.console_hint_options));
            // The hero's own lines leave the ambient on the hero's game.
            if (!HomeHero.SHELF.equals(shelf)) ambience.onCardFocus(shelf, item, card);
        };
        if (shelves != null) {
            shelves.setFocusListener(focus);
        } else {
            adapter.setFocusListener(focus);
        }
        list.getViewTreeObserver().addOnTouchModeChangeListener(touch -> {
            if (touch || hints.source() != InputSource.NONE) return;
            onInput(InputSource.leavingTouch(com.limelight.ui.StreamMenuHint.hasGamepad(),
                    getResources().getConfiguration().keyboard == android.content.res.Configuration.KEYBOARD_QWERTY), 0);
        });
        // Focus on the chrome has nothing for the hints to say.
        list.getViewTreeObserver().addOnGlobalFocusChangeListener((before, now) -> {
            if (now == null || !isInList(now)) hints.onFocus(null, null);
        });
    }

    private boolean isInList(View view) {
        for (android.view.ViewParent up = view.getParent(); up != null; up = up.getParent()) {
            if (up == list) return true;
        }
        return false;
    }

    /** The last input names the keys in the hints: on the next input, with no animation. */
    private void onInput(InputSource source, int vendor) {
        if (source == null) return;
        InputSource.remember(source);
        if (ambience != null) ambience.setInput(source, vendor);
        if (hints != null) hints.onInput(source, vendor);
    }

    @Override
    public boolean dispatchTouchEvent(android.view.MotionEvent event) {
        if (event.getActionMasked() == android.view.MotionEvent.ACTION_DOWN) {
            atStart = false;
            onInput(InputSource.of(event), 0);
        }
        return super.dispatchTouchEvent(event);
    }

    @Override
    public boolean dispatchGenericMotionEvent(android.view.MotionEvent event) {
        if (InputSource.of(event) != null) atStart = false;
        onInput(InputSource.of(event), event.getDevice() == null ? 0 : event.getDevice().getVendorId());
        return super.dispatchGenericMotionEvent(event);
    }

    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent event) {
        if (event.getAction() == android.view.KeyEvent.ACTION_DOWN) {
            atStart = false;
            // The key that ends touch mode never gets here: focus stands, and this one moves it. A refresh must not take it back.
            if (com.limelight.ui.InputSource.isDpad(event.getKeyCode())) focusPending = false;
            onInput(InputSource.of(event), InputSource.vendorOf(event));
        }
        // Up from the top of the list goes to the current tab of the shell bar, whatever the focus search would pick
        // (View, at the bar's end, included).
        // On a phone the tabs are at the foot: Up from the top card stays in the top row.
        if (chrome != null && chrome.form() != com.limelight.ui.GlassForm.PHONE
                && com.limelight.ui.ShellFocus.upToCurrentTab(this, event, list, findViewById(R.id.console_view))) {
            return true;
        }
        return super.dispatchKeyEvent(event);
    }

    @Override
    protected void onResume() {
        super.onResume();
        shown = true;
        ShellBar.refresh(this);
        feed.prune();
        feed.resume();
        // A game may have started or ended while another screen was in front.
        refreshedSignature = null;
        handler.removeCallbacks(settle);
        handler.postDelayed(settle, SETTLE_MS);
        handler.removeCallbacks(stopWaiting);
        if (feed.waiting()) {
            handler.postDelayed(stopWaiting, WAIT_LIMIT_MS);
        }
        scheduleReload();
    }

    @Override
    protected void onPause() {
        shown = false;
        pairingWatch.pause();
        wake.cancel();
        feed.pause();
        handler.removeCallbacks(reload);
        handler.removeCallbacks(settle);
        handler.removeCallbacks(stopWaiting);
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        handler.removeCallbacksAndMessages(null);
        hostActions.destroy();
        feed.unbind();
        worker.shutdownNow();
        starWorker.shutdownNow();
        if (chrome != null) chrome.destroy();
        if (ambience != null) {
            ambience.destroy();
        }
        if (covers != null) {
            covers.release();
        }
        super.onDestroy();
    }

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        UiHelper.notifyNewRootView(this);
        lastWidthPx = 0;
    }

    @Override
    public void onHostsChanged() {
        // Every poll reports its PC, changed or not; only news is worth reading the library and binding the cards for.
        if (drawn.changed(feed.hosts(), feed.waiting())) {
            scheduleReload();
        }
        // The watch has no timer of its own: it counts on every report.
        if (shown) {
            pairingWatch.poll(feed.hosts());
        }
    }

    /** "TV wants to pair" while something waits. */
    protected void showWaitingToPair(List<PairingWatch.Waiting> waiting) {
        waitingToPair = waiting;
        pairingBanner.setVisibility(waiting.isEmpty() ? View.GONE : View.VISIBLE);
        if (waiting.size() == 1) {
            pairingBanner.setText(getString(R.string.pairing_wants_to_pair, waiting.get(0).shownName()));
        } else if (!waiting.isEmpty()) {
            pairingBanner.setText(getString(R.string.pairing_many_want_to_pair, waiting.size()));
        }
    }

    private void scheduleReload() {
        handler.removeCallbacks(reload);
        handler.postDelayed(reload, DEBOUNCE_MS);
    }

    /** What changes how Home looks or what it asks the hosts: a host's identity, state, address and game. */
    static String signature(List<ComputerDetails> hosts) {
        StringBuilder sb = new StringBuilder();
        for (ComputerDetails host : hosts) {
            sb.append(host.uuid).append('|').append(host.displayName()).append('|').append(host.state).append('|')
                    .append(host.activeAddress == null ? "" : host.activeAddress.address + ":" + host.activeAddress.port)
                    .append('|').append(host.runningGameId).append('|').append(host.runningGameUuid).append(';');
        }
        return sb.toString();
    }

    private void reloadNow() {
        if (!shown || isFinishing() || isDestroyed()) {
            return;
        }
        String uniqueId = feed.uniqueId();
        if (uniqueId == null) {
            // No manager to read the PCs from; once Home stops waiting for it, it says there are none.
            if (!feed.waiting() && feed.binder() == null) {
                show(++generation, ConsoleSnapshot.empty(), HomeModel.empty(), true, true);
            }
            return;
        }
        if (repository == null) {
            repository = createRepository(uniqueId);
            covers = new CoverLoaders(this, uniqueId, COVER_WIDTH_DP);
            if (ambience != null) covers.setArtListener(ambience::onArt);
            if (searchEnv != null) searchEnv.covers = covers;
            if (shelves != null) {
                shelves.setCovers(covers);
            } else {
                adapter.setCovers(covers);
            }
        }
        final ConsoleRepository repo = repository;
        final List<ComputerDetails> hosts = feed.hosts();
        drawn.set(hosts, feed.waiting());
        final String signature = signature(hosts);
        final boolean refresh = !signature.equals(refreshedSignature);
        refreshedSignature = signature;
        final int mine = ++generation;
        final Set<String> online = onlineOf(hosts);
        if (refresh) {
            loading.setVisibility(View.VISIBLE);
        }
        worker.execute(() -> {
            try {
                Set<String> denied = HomeModel.launchDenied(hosts);
                ConsoleSnapshot saved = repo.cached(hosts);
                show(mine, saved, HomeModel.of(saved, online, denied), hosts.isEmpty(), !refresh);
                if (refresh) {
                    ConsoleSnapshot fresh = repo.refresh(hosts);
                    show(mine, fresh, HomeModel.of(fresh, online, denied), hosts.isEmpty(), true);
                    redrawAtNextPoll(mine);
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("Home could not load: " + e);
                show(mine, ConsoleSnapshot.empty(), HomeModel.empty(), hosts.isEmpty(), true);
                redrawAtNextPoll(mine);
            }
        });
    }

    /** A read of the PCs changed what is saved, and a failed load drew nothing: the next poll draws again, once. */
    private void redrawAtNextPoll(int mine) {
        runOnUiThread(() -> {
            if (mine == generation) {
                drawn.forget();
            }
        });
    }

    private static Set<String> onlineOf(List<ComputerDetails> hosts) {
        Set<String> online = new HashSet<>();
        for (ComputerDetails host : hosts) {
            if (host.state == ComputerDetails.State.ONLINE) {
                online.add(host.uuid);
            }
        }
        return online;
    }

    private void show(int mine, ConsoleSnapshot snapshot, HomeModel model, boolean noHosts, boolean done) {
        runOnUiThread(() -> {
            if (mine != generation || isFinishing() || isDestroyed()) {
                return;
            }
            shownSnapshot = snapshot;
            this.model = model;
            if (capsule != null) capsule.show(model);
            submitKeepingFocus(model);
            if (search != null) search.onModel();
            // While a saved PC has yet to answer, an empty Home is still loading, not "No PCs yet".
            boolean waiting = model.isEmpty() && noHosts && feed.waiting();
            if (done && !waiting) {
                loading.setVisibility(View.GONE);
            }
            // "No hosts" waits until the hosts had time to answer, so it does not flash on every start.
            boolean showEmpty = model.isEmpty() && noHosts && settled && !waiting;
            empty.setVisibility(showEmpty ? View.VISIBLE : View.GONE);
            if (search == null || !search.isOpen()) list.setVisibility(showEmpty ? View.INVISIBLE : View.VISIBLE);
            focusFirstCard(showEmpty);
        });
    }

    /** With a D-pad or controller the first card has focus from the start, so a press opens something. */
    private void focusFirstCard(boolean emptyShown) {
        if (!focusPending && !atStart) {
            return;
        }
        if (emptyShown) {
            focusPending = false;
            findViewById(R.id.console_empty_hosts).requestFocus();
            return;
        }
        if (shelves == null) {
            int first = adapter.initialFocus();
            if (ambience != null && first < 0) {
                ambience.clear();
            }
            if (first < 0 || list.isInTouchMode()) {
                // TV: no card takes focus until a key is pressed, but the hero shows the one focus would start on.
                if (ambience != null && first >= 0 && list.getFocusedChild() == null) ambience.refresh(adapter.itemAt(first));
                return;
            }
            focusPending = false;
            list.post(() -> {
                // Asked again here: the hosts' answer may have changed the list since.
                int now = adapter.initialFocus();
                if (now < 0) return;
                RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(now);
                if (holder != null) {
                    holder.itemView.requestFocus();
                } else {
                    grid.scrollToPosition(now);
                    focusPending = true;
                }
            });
            return;
        }
        ShelfAdapter.Item hero = shelves.heroItem();
        if (hero != null) {
            // The hero's game holds the ambient until focus goes elsewhere, and a key starts on its Play.
            if (ambience != null && shelves.focusedCard(list) == null) ambience.refresh(hero);
            if (list.isInTouchMode()) return;
            focusPending = false;
            list.post(() -> {
                View play = shelves.heroPlay(list);
                if (play != null) play.requestFocus();
                else focusPending = true;
            });
            return;
        }
        ConsoleHomeAdapter.Cursor first = shelves.initialFocus();
        if (first == null) {
            if (ambience != null) ambience.clear();
            return;
        }
        if (list.isInTouchMode()) {
            // No card takes focus until a key is pressed, but the hero is not left blank: it shows the card focus would start on.
            if (ambience != null && shelves.focusedCard(list) == null) ambience.refresh(shelves.itemAt(first));
            return;
        }
        focusPending = false;
        restoreFocus(first, 0);
    }

    /**
     * Focuses the card once its shelf and it are laid out; a card out of range is brought in and asked for again. A place on the
     * list means nothing once the list has changed, so the card goes by its keys, and at the start it is whatever starts now.
     */
    private void restoreFocus(ConsoleHomeAdapter.Cursor target, int attempt) {
        restoreFocus(shelves.rowKey(target.row), shelves.keyAt(target), attempt);
    }

    private void restoreFocus(String shelf, String key, int attempt) {
        list.post(() -> {
            ConsoleHomeAdapter.Cursor target = atStart ? shelves.initialFocus() : shelves.positionOfKey(shelf, key);
            if (target == null || attempt >= FOCUS_TRIES) return;
            if (list.hasPendingAdapterUpdates()) {
                // The list is about to lay out what it was just given: a frame from now it knows its cards.
                list.postOnAnimation(() -> restoreFocus(shelf, key, attempt + 1));
                return;
            }
            View card = shelves.cardView(list, target);
            if (card != null) {
                if (!card.hasFocus()) {
                    card.requestFocus();
                } else if (ambience != null) {
                    // The card kept focus through the refresh: its state may have changed, so the hero is written again.
                    ambience.refresh(shelves.itemAt(target));
                }
            } else {
                shelves.reveal(list, target);
                restoreFocus(shelf, key, attempt + 1);
            }
        });
    }

    private static final int FOCUS_TRIES = 8;

    /** What {@link #model} was made from: the hero's Play reads it as the game's page reads its own. */
    private ConsoleSnapshot shownSnapshot = ConsoleSnapshot.empty();

    private void openGame(HomeModel.GameCard card) {
        // A game in several PCs' libraries asks which one first; a found game that is not added opens as before.
        if (!card.notAdded() && card.hostCount > 1 && askHost(card)) {
            return;
        }
        GameOpener.open(opener, worker, card);
    }

    /** Starts or resumes the game from the hero; a game that needs its page first opens that. */
    private void playHero(HomeModel.GameCard card) {
        ComputerDetails host = feed.host(card.hostUuid);
        if (HomeHero.opensPage(card, host)) {
            openGame(card);
            return;
        }
        if (feed.binder() == null) {
            hostActions.showError(host.displayName(), getString(R.string.error_manager_not_running), null);
            return;
        }
        GameDetailModel page = HomeHero.modelOf(card, host, shownSnapshot);
        if (!page.canStart()) {
            // The page says why not.
            openGame(card);
            return;
        }
        com.limelight.nvstream.http.NvApp app = CoverLoaders.appOf(card.title, card.appId, card.source);
        PlayPlan plan = HomeHero.planOf(page, app, host);
        if (plan.kind == PlayPlan.Kind.START) {
            play(app, host, card.badge == HomeModel.Badge.NONE ? LaunchSource.CONSOLE_PLAY : LaunchSource.CONSOLE_RESUME);
            return;
        }
        // Starting closes what runs there, so it asks as the game's page does.
        final String hostUuid = card.hostUuid;
        UiHelper.showDialog(this, ReplaceRunningPrompt.ask(this, plan, page.title, app, new ReplaceRunningPrompt.Screen() {
            @Override public ComputerDetails currentHost() {
                return feed.host(hostUuid);
            }

            @Override public void start(com.limelight.nvstream.http.NvApp starting, ComputerDetails on) {
                play(starting, on, LaunchSource.CONSOLE_PLAY);
            }

            @Override public void resume(com.limelight.nvstream.http.NvApp running, ComputerDetails on) {
                play(running, on, LaunchSource.CONSOLE_RESUME);
            }

            @Override public void close(com.limelight.nvstream.http.NvApp running, ComputerDetails on, java.util.function.Consumer<Boolean> onDone) {
                ConsoleHomeActivity.this.close(running, on, onDone);
            }

            @Override public void reload() {
                refreshedSignature = null;
                scheduleReload();
            }
        }));
    }

    /** Asks, then ends the game on its PC; Home reads the PC again once that answered. */
    private void quitHero(HomeModel.GameCard card) {
        ComputerDetails host = feed.host(card.hostUuid);
        if (host == null) return;
        if (feed.binder() == null) {
            hostActions.showError(host.displayName(), getString(R.string.error_manager_not_running), null);
            return;
        }
        UiHelper.displayQuitConfirmationDialog(this, () -> end(CoverLoaders.appOf(card.title, card.appId, card.source), host, () ->
                handler.post(() -> {
                    refreshedSignature = null;
                    scheduleReload();
                })), null);
    }

    /** Starts the stream; the debug demo overrides this so no stream is opened. */
    protected void play(com.limelight.nvstream.http.NvApp app, ComputerDetails computer, String source) {
        ServerHelper.doStart(this, app, computer, feed.binder(), source);
    }

    /** Ends the game on the host, then calls {@code onComplete} on any thread. */
    protected void end(com.limelight.nvstream.http.NvApp app, ComputerDetails computer, Runnable onComplete) {
        ServerHelper.doQuit(this, computer, app, feed.binder(), onComplete);
    }

    /** Closes the game on the host; {@code onDone} learns on any thread whether it is closed. */
    protected void close(com.limelight.nvstream.http.NvApp app, ComputerDetails computer, java.util.function.Consumer<Boolean> onDone) {
        ServerHelper.doQuitReporting(this, computer, app, feed.binder(), onDone);
    }

    private void openGameOn(String hostUuid, int appId, String title, String hostName) {
        startActivity(new Intent(this, detailActivity())
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, hostUuid)
                .putExtra(GameDetailActivity.EXTRA_APP_ID, appId)
                .putExtra(GameDetailActivity.EXTRA_TITLE, title)
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, hostName));
    }

    /**
     * A game that several PCs have asks which one to open it on, from the saved libraries.
     *
     * @return false when there is nothing to ask (no library read yet): the card's own PC opens instead
     */
    private boolean askHost(HomeModel.GameCard card) {
        final ConsoleRepository repo = repository;
        final String libraryKey = card.libraryKey;
        if (repo == null || libraryKey == null) {
            return false;
        }
        if (choosing) {
            return true;
        }
        choosing = true;
        final List<ComputerDetails> hosts = feed.hosts();
        final Set<String> online = onlineOf(hosts);
        // The saved libraries are on disk: read them on the star thread, not behind a refresh from the hosts.
        starWorker.execute(() -> {
            List<HostChoice> choices;
            try {
                choices = HostChoice.of(repo.cached(hosts), libraryKey, online);
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("Home could not read the saved libraries: " + e);
                choices = Collections.emptyList();
            }
            final List<HostChoice> found = choices;
            runOnUiThread(() -> {
                if (!shown || isFinishing() || isDestroyed()) {
                    choosing = false;
                } else if (found.size() > 1) {
                    HostChooser.show(this, found, chosen ->
                            openGameOn(chosen.hostUuid, chosen.appId, card.title, chosen.hostName), () -> choosing = false);
                } else {
                    choosing = false;
                    openGameOn(card.hostUuid, card.appId, card.title, card.hostName);
                }
            });
        });
        return true;
    }

    /** Sends the wake packets to a PC that is away (off the UI thread); the debug demo and tests override this. */
    protected void sendWake(ComputerDetails computer) throws java.io.IOException {
        com.limelight.nvstream.wol.WakeOnLanSender.sendWolPacket(this, computer);
    }

    /** The games this screen is adding on a host now; see {@link GameOpener.Screen#adding}. */
    private final Set<String> adding = new HashSet<>();
    private final GameOpener.Screen opener = new GameOpener.Screen() {
        @Override public android.app.Activity activity() { return ConsoleHomeActivity.this; }
        @Override public ConsoleRepository repository() { return repository; }
        @Override public ComputerDetails host(String uuid) { return feed.host(uuid); }
        @Override public Class<? extends GameDetailActivity> detailActivity() { return ConsoleHomeActivity.this.detailActivity(); }
        @Override public Set<String> adding() { return adding; }
        @Override public void onAdded() {
            refreshedSignature = null;
            scheduleReload();
            // A form change waited for the add.
            followFormSoon();
        }
    };

    /** Submits the model; when a card had focus, the same card (by its stable id) has it afterwards. */
    private void submitKeepingFocus(HomeModel model) {
        if (shelves == null) {
            submitGridKeepingFocus(model);
            return;
        }
        if (atStart) {
            // Nobody chose anything yet: the shelves start over, and focusFirstCard places the start.
            shelves.forgetScroll();
            shelves.submit(model);
            list.scrollToPosition(0);
            return;
        }
        ConsoleHomeAdapter.Cursor at = shelves.focusedCard(list);
        String key = shelves.keyAt(at);
        String shelf = at == null ? null : shelves.rowKey(at.row);
        shelves.submit(model);
        if (key == null) {
            return;
        }
        // A card that left (an unstarred favourite) hands focus to its neighbour instead of dropping it.
        ConsoleHomeAdapter.Cursor target = shelves.positionOfKey(shelf, key);
        if (target == null) {
            target = shelves.nearestCard(at);
        }
        if (target == null) {
            return;
        }
        restoreFocus(target, 0);
    }

    private void submitGridKeepingFocus(HomeModel model) {
        View focused = list.getFocusedChild();
        int at = focused == null ? -1 : list.getChildAdapterPosition(focused);
        String key = at < 0 ? null : adapter.keyAt(at);
        adapter.submit(model, view.group);
        if (key == null) {
            return;
        }
        int target = adapter.positionOfKey(key);
        if (target < 0) {
            target = adapter.nearestCard(at);
        }
        if (target < 0) {
            return;
        }
        final int position = target;
        list.post(() -> {
            RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(position);
            if (holder != null) {
                if (!holder.itemView.hasFocus()) {
                    holder.itemView.requestFocus();
                }
            } else {
                grid.scrollToPosition(position);
                list.post(() -> {
                    RecyclerView.ViewHolder later = list.findViewHolderForAdapterPosition(position);
                    if (later != null) {
                        later.itemView.requestFocus();
                    }
                });
            }
        });
    }

    /** The long-press menu of a game card: star or unstar it, hide it. A game not added has no app id for either. */
    void askFavourite(HomeModel.GameCard card) {
        if (repository == null || card.notAdded()) {
            return;
        }
        boolean remove = card.favourite;
        List<ConsoleSnapshot.Source> copies = model.copiesOf(card);
        CharSequence star = getString(remove ? R.string.console_favourite_remove : R.string.console_favourite_add);
        // A running game stays under Continue playing while hidden: its menu shows it again.
        boolean hide = !card.sourceHidden();
        CharSequence hiding = getString(hide ? R.string.applist_menu_hide_app : R.string.applist_menu_show_app);
        CharSequence openPlay = OpenPlay.menuLabel(this, card.hostUuid, card.appId);
        List<CharSequence> labels = new java.util.ArrayList<>();
        List<Runnable> actions = new java.util.ArrayList<>();
        // A phone's hero has room for one button, Play: Quit game is here, on Hold OK and Y too.
        if (card.badge == HomeModel.Badge.RUNNING && !OfflineLook.allAway(card.hostUuids, feed::host)) {
            ComputerDetails host = feed.host(card.hostUuid);
            if (host != null && host.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
                labels.add(getString(R.string.applist_menu_quit));
                actions.add(() -> quitHero(card));
            }
        }
        labels.add(star);
        actions.add(() -> setFavourite(card, !remove));
        labels.add(openPlay);
        actions.add(() -> OpenPlay.toggle(this, card.hostUuid, card.hostName, card.appId, card.source != null ? card.source.stableId() : "", card.title));
        if (!copies.isEmpty()) {
            labels.add(hiding);
            actions.add(() -> setHidden(copies, hide));
        }
        UiHelper.showDialog(this, new com.limelight.ui.NavigationDialogBuilder(this)
                .setTitle(card.title)
                .setItems(labels.toArray(new CharSequence[0]), (dialog, which) -> actions.get(which).run())
                .setNegativeButton(android.R.string.cancel, null).create());
    }

    /** Hides or shows on the star thread and paints the saved result right away, like a star. */
    private void setHidden(List<ConsoleSnapshot.Source> copies, boolean hide) {
        final ConsoleRepository repo = repository;
        if (repo == null) {
            return;
        }
        final List<ComputerDetails> hosts = feed.hosts();
        final Set<String> online = onlineOf(hosts);
        starWorker.execute(() -> {
            try {
                repo.setHidden(copies, hide);
                HomeModel model = HomeModel.of(repo.cached(hosts), online, HomeModel.launchDenied(hosts));
                runOnUiThread(() -> {
                    if (isFinishing() || isDestroyed()) {
                        return;
                    }
                    this.model = model;
                    submitKeepingFocus(model);
                    for (ConsoleSnapshot.Source copy : copies) {
                        com.limelight.utils.FrontendSync.appListChanged(this, feed.host(copy.hostUuid));
                    }
                    scheduleReload();
                });
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("Home could not hide a game: " + e);
            }
        });
    }

    /**
     * Stars or unstars on the star thread and paints the saved result right away, from the stored library;
     * the normal reload follows and takes over.
     */
    private void setFavourite(HomeModel.GameCard card, boolean starred) {
        final ConsoleRepository repo = repository;
        if (repo == null || card.notAdded()) {
            return;
        }
        final List<ComputerDetails> hosts = feed.hosts();
        final Set<String> online = onlineOf(hosts);
        final String key = card.source != null ? card.source.stableId() : "";
        starWorker.execute(() -> {
            try {
                repo.setFavourite(card.hostUuid, card.hostName, key, card.appId, card.title, starred);
                HomeModel model = HomeModel.of(repo.cached(hosts), online, HomeModel.launchDenied(hosts));
                runOnUiThread(() -> {
                    if (isFinishing() || isDestroyed()) {
                        return;
                    }
                    this.model = model;
                    submitKeepingFocus(model);
                    scheduleReload();
                });
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("Home could not update a favourite: " + e);
            }
        });
    }

    /** The screen a game opens; the debug demo returns its own. */
    protected Class<? extends GameDetailActivity> detailActivity() {
        return GameDetailActivity.class;
    }

    /** The screen an "All N" tile opens; the debug demo returns its own. */
    protected Class<? extends AllGamesActivity> allGamesActivity() {
        return AllGamesActivity.class;
    }

    private void openHost(HomeModel.HostCard card) {
        if (!card.online) {
            // A PC that is away is woken from here; one that cannot be (no MAC address yet) goes to the list of PCs.
            if (!wake.wake(card.uuid)) {
                openHosts();
            }
            return;
        }
        openLibrary(card.uuid, card.name);
    }

    private void openLibrary(String uuid, String name) {
        HostCapsule.openLibrary(this, uuid, name);
    }

    /** The list of PCs: pairing and adding a PC. */
    private void openHosts() {
        ShellBar.open(this, ShellBar.Destination.HOME, ShellBar.Destination.PCS);
    }
}
