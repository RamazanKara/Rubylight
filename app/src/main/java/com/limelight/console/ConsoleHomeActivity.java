package com.limelight.console;

import android.content.Intent;
import android.content.res.Configuration;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.View;

import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.appbar.MaterialToolbar;
import com.limelight.AppView;
import com.limelight.R;
import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.StreamSettings;
import com.limelight.ui.NightjarActivity;
import com.limelight.ui.TopAppBar;
import com.limelight.utils.UiHelper;

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
    private static final int COVER_WIDTH_DP = 140;

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor(r -> {
        Thread thread = new Thread(r, "console-home");
        thread.setDaemon(true);
        return thread;
    });
    private HostFeed feed;
    private ConsoleHomeAdapter adapter;
    private CoverLoaders covers;
    private RecyclerView list;
    private GridLayoutManager grid;
    private View loading;
    private View empty;
    private volatile ConsoleRepository repository;
    private int generation;
    private String refreshedSignature;
    private boolean settled;
    private boolean shown;
    private boolean focusPending = true;
    private int columns = 2;
    private int lastWidthPx;

    private final Runnable reload = this::reloadNow;
    private final Runnable settle = () -> {
        settled = true;
        reloadNow();
    };

    /** The data behind the screen; the debug demo overrides this with canned hosts. */
    protected ConsoleRepository createRepository(String uniqueId) {
        return ConsoleRepository.create(this, uniqueId);
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        setContentView(R.layout.activity_console_home);
        UiHelper.notifyNewRootView(this);

        MaterialToolbar toolbar = TopAppBar.configure(this, R.id.console_toolbar, getString(R.string.console_title),
                R.menu.console_home);
        toolbar.setOnMenuItemClickListener(item -> {
            if (item.getItemId() == R.id.console_hosts_button) {
                openHosts();
                return true;
            }
            if (item.getItemId() == R.id.settingsButton) {
                startActivity(new Intent(this, StreamSettings.class));
                return true;
            }
            return false;
        });

        loading = findViewById(R.id.console_loading);
        empty = findViewById(R.id.console_empty);
        findViewById(R.id.console_empty_hosts).setOnClickListener(v -> openHosts());

        list = findViewById(R.id.console_list);
        grid = new GridLayoutManager(this, columns);
        grid.setSpanSizeLookup(new GridLayoutManager.SpanSizeLookup() {
            @Override
            public int getSpanSize(int position) {
                return spanOf(adapter.viewTypeAt(position), columns);
            }
        });
        list.setLayoutManager(grid);
        list.setItemAnimator(null);
        list.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> fitColumns(r - l));

        feed = new HostFeed(this, this);
        adapter = new ConsoleHomeAdapter(this, null, feed::host, new ConsoleHomeAdapter.Listener() {
            @Override
            public void onGame(HomeModel.GameCard card) {
                openGame(card);
            }

            @Override
            public void onHost(HomeModel.HostCard card) {
                openHost(card);
            }
        });
        list.setAdapter(adapter);
        feed.bind();
    }

    /** Headers take the whole row; wide cards take two columns, or the whole row when it has at most three. */
    static int spanOf(int viewType, int columns) {
        switch (viewType) {
            case ConsoleHomeAdapter.HEADER:
                return columns;
            case ConsoleHomeAdapter.COVER_GAME:
                return 1;
            default:
                return columns <= 3 ? columns : 2;
        }
    }

    /** Columns for a list this wide: about 130 dp each, and an even number from four on so wide cards pair up. */
    static int columnsFor(int widthDp) {
        int columns = Math.max(2, widthDp / 130);
        return columns >= 4 && columns % 2 == 1 ? columns - 1 : columns;
    }

    private void fitColumns(int widthPx) {
        if (widthPx <= 0 || widthPx == lastWidthPx) {
            return;
        }
        lastWidthPx = widthPx;
        float density = getResources().getDisplayMetrics().density;
        int count = columnsFor(Math.round(widthPx / density));
        int margin = getResources().getDimensionPixelSize(R.dimen.space_s);
        int columnWidth = (widthPx - list.getPaddingLeft() - list.getPaddingRight()) / count - 2 * margin;
        list.post(() -> {
            if (count != columns) {
                columns = count;
                grid.setSpanCount(count);
            }
            adapter.setCoverHeight(Math.max(1, columnWidth * 4 / 3));
        });
    }

    @Override
    protected void onResume() {
        super.onResume();
        shown = true;
        feed.resume();
        // A game may have started or ended while another screen was in front.
        refreshedSignature = null;
        handler.removeCallbacks(settle);
        handler.postDelayed(settle, SETTLE_MS);
        scheduleReload();
    }

    @Override
    protected void onPause() {
        shown = false;
        feed.pause();
        handler.removeCallbacks(reload);
        handler.removeCallbacks(settle);
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        handler.removeCallbacksAndMessages(null);
        feed.unbind();
        worker.shutdownNow();
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
        scheduleReload();
    }

    private void scheduleReload() {
        handler.removeCallbacks(reload);
        handler.postDelayed(reload, DEBOUNCE_MS);
    }

    /** What changes how Home looks or what it asks the hosts: a host's identity, state, address and game. */
    static String signature(List<ComputerDetails> hosts) {
        StringBuilder sb = new StringBuilder();
        for (ComputerDetails host : hosts) {
            sb.append(host.uuid).append('|').append(host.name).append('|').append(host.state).append('|')
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
            return;
        }
        if (repository == null) {
            repository = createRepository(uniqueId);
            covers = new CoverLoaders(this, uniqueId, COVER_WIDTH_DP);
            adapter.setCovers(covers);
        }
        final ConsoleRepository repo = repository;
        final List<ComputerDetails> hosts = feed.hosts();
        final String signature = signature(hosts);
        final boolean refresh = !signature.equals(refreshedSignature);
        refreshedSignature = signature;
        final int mine = ++generation;
        final Set<String> online = new HashSet<>();
        for (ComputerDetails host : hosts) {
            if (host.state == ComputerDetails.State.ONLINE) {
                online.add(host.uuid);
            }
        }
        if (refresh) {
            loading.setVisibility(View.VISIBLE);
        }
        worker.execute(() -> {
            try {
                show(mine, HomeModel.of(repo.cached(hosts), online), hosts.isEmpty(), !refresh);
                if (refresh) {
                    ConsoleSnapshot fresh = repo.refresh(hosts);
                    show(mine, HomeModel.of(fresh, online), hosts.isEmpty(), true);
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            } catch (RuntimeException e) {
                com.limelight.LimeLog.warning("Home could not load: " + e);
                show(mine, HomeModel.empty(), hosts.isEmpty(), true);
            }
        });
    }

    private void show(int mine, HomeModel model, boolean noHosts, boolean done) {
        runOnUiThread(() -> {
            if (mine != generation || isFinishing() || isDestroyed()) {
                return;
            }
            adapter.submit(model);
            if (done) {
                loading.setVisibility(View.GONE);
            }
            // "No hosts" waits until the hosts had time to answer, so it does not flash on every start.
            boolean showEmpty = model.isEmpty() && noHosts && settled;
            empty.setVisibility(showEmpty ? View.VISIBLE : View.GONE);
            list.setVisibility(showEmpty ? View.INVISIBLE : View.VISIBLE);
            focusFirstCard(showEmpty);
        });
    }

    /** With a D-pad or controller the first card has focus from the start, so a press opens something. */
    private void focusFirstCard(boolean emptyShown) {
        if (!focusPending) {
            return;
        }
        if (emptyShown) {
            focusPending = false;
            findViewById(R.id.console_empty_hosts).requestFocus();
            return;
        }
        int first = adapter.firstCard();
        if (first < 0 || list.isInTouchMode()) {
            return;
        }
        focusPending = false;
        list.post(() -> {
            RecyclerView.ViewHolder holder = list.findViewHolderForAdapterPosition(first);
            if (holder != null) {
                holder.itemView.requestFocus();
            } else {
                grid.scrollToPosition(first);
                focusPending = true;
            }
        });
    }

    private void openGame(HomeModel.GameCard card) {
        startActivity(new Intent(this, detailActivity())
                .putExtra(GameDetailActivity.EXTRA_HOST_UUID, card.hostUuid)
                .putExtra(GameDetailActivity.EXTRA_APP_ID, card.appId)
                .putExtra(GameDetailActivity.EXTRA_TITLE, card.title)
                .putExtra(GameDetailActivity.EXTRA_HOST_NAME, card.hostName));
    }

    /** The screen a game opens; the debug demo returns its own. */
    protected Class<? extends GameDetailActivity> detailActivity() {
        return GameDetailActivity.class;
    }

    private void openHost(HomeModel.HostCard card) {
        if (!card.online) {
            openHosts();
            return;
        }
        startActivity(new Intent(this, AppView.class)
                .putExtra(AppView.NAME_EXTRA, card.name)
                .putExtra(AppView.UUID_EXTRA, card.uuid));
    }

    /** The classic list of PCs: pairing, wake, host settings. */
    private void openHosts() {
        startActivity(HomeSettings.pcList(this));
    }
}
