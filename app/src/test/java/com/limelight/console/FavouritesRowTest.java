package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.view.ContextThemeWrapper;
import android.widget.FrameLayout;

import androidx.recyclerview.widget.RecyclerView;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiFixtures;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;

/** The Favourites row of Home and the menu on a long press (a held D-pad centre is a long press too). */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class FavouritesRowTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private ConsoleRig rig;
    private ContextThemeWrapper context;

    @Before
    public void setUp() throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
        context = new ContextThemeWrapper(RuntimeEnvironment.getApplication(), R.style.AppTheme);
        rig.transport.reply("tower", "library", ok(library("t1",
                game("t-portal", 11, "Portal", "steam:400", false), game("t-zelda", 12, "Zelda", null, false))));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
    }

    @After
    public void tearDown() {
        rig.close();
    }

    private HomeModel model() throws Exception {
        return HomeModel.of(rig.repository.refresh(Collections.singletonList(nativeHost("tower", "Tower", true))),
                new HashSet<>(Collections.singletonList("tower")));
    }

    @Test
    public void theFavouritesRowComesBeforeContinuePlayingAndTheLibrary() throws Exception {
        rig.recents.add(new com.limelight.nativeapi.RecentsStore.Recent("tower", "Tower", 11, "Portal", 10));
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        List<RecordedMenu> menus = new ArrayList<>();
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(menus));
        adapter.submit(model());

        // Header, Zelda (favourite), header, Portal (recent), header, library, header, hosts.
        assertEquals(ConsoleHomeAdapter.HEADER, adapter.viewTypeAt(0));
        assertEquals(ConsoleHomeAdapter.COVER_GAME, adapter.viewTypeAt(1));
        assertEquals(ConsoleHomeAdapter.HEADER, adapter.viewTypeAt(2));
        assertEquals(ConsoleHomeAdapter.WIDE_GAME, adapter.viewTypeAt(3));
        FrameLayout parent = new FrameLayout(context);
        RecyclerView.ViewHolder first = adapter.onCreateViewHolder(parent, ConsoleHomeAdapter.HEADER);
        adapter.onBindViewHolder(first, 0);
        assertEquals(context.getString(R.string.console_section_favourites),
                ((android.widget.TextView) first.itemView).getText().toString());
        assertEquals(1, adapter.firstCard());
    }

    @Test
    public void aLongPressOnAGameCardOpensItsMenuAndAHostCardHasNone() throws Exception {
        rig.repository.setFavourite("tower", "Tower", 12, "Zelda", true);
        List<RecordedMenu> menus = new ArrayList<>();
        ConsoleHomeAdapter adapter = new ConsoleHomeAdapter(context, null, uuid -> null, recorder(menus));
        HomeModel model = model();
        adapter.submit(model);
        FrameLayout parent = new FrameLayout(context);

        RecyclerView.ViewHolder card = adapter.onCreateViewHolder(parent, ConsoleHomeAdapter.COVER_GAME);
        adapter.onBindViewHolder(card, 1);
        assertTrue(card.itemView.isFocusable());
        assertTrue(card.itemView.performLongClick());
        assertEquals(1, menus.size());
        assertEquals("Zelda", menus.get(0).title);
        assertTrue(menus.get(0).favourite);
        assertTrue(card.itemView.getContentDescription().toString()
                .endsWith(context.getString(R.string.console_favourite_description)));

        // The same holder, recycled for a host card, no longer reacts to a long press.
        RecyclerView.ViewHolder host = adapter.onCreateViewHolder(parent, ConsoleHomeAdapter.HOST);
        adapter.onBindViewHolder(host, adapter.getItemCount() - 1);
        assertFalse(host.itemView.isLongClickable());
        assertEquals(1, menus.size());
    }

    private static final class RecordedMenu {
        final String title;
        final boolean favourite;

        RecordedMenu(String title, boolean favourite) {
            this.title = title;
            this.favourite = favourite;
        }
    }

    private static ConsoleHomeAdapter.Listener recorder(List<RecordedMenu> menus) {
        return new ConsoleHomeAdapter.Listener() {
            @Override public void onGame(HomeModel.GameCard card) {
            }

            @Override public void onGameMenu(HomeModel.GameCard card) {
                menus.add(new RecordedMenu(card.title, card.favourite));
            }

            @Override public void onHost(HomeModel.HostCard card) {
            }
        };
    }
}
