package com.limelight.console;

import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.TextView;

import androidx.core.content.ContextCompat;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.StreamProfilePicker;
import com.limelight.ui.ActionSheet;
import com.limelight.ui.AppTheme;
import com.limelight.ui.BackdropView;
import com.limelight.ui.ShellBar;
import com.limelight.ui.TopAppBar;

/**
 * View binding, ambient backdrop, chip line, facts lookup, profile row and quit-on-leave rendering
 * for {@link GameDetailActivity}.
 */
final class GameDetailView {
    private final GameDetailActivity activity;

    private View loading;
    private ImageView cover;
    private TextView coverText;
    private TextView title;
    private TextView status;
    private TextView host;
    /** Genres, themes and time to beat from IGDB; gone without a record or IGDB keys. */
    private TextView factsView;
    private String factsShownFor;
    private TextView note;
    private MaterialButton play;
    private MaterialButton end;
    private MaterialButton favourite;
    private View profileRow;
    private TextView profileValues;
    private TextView profileLayers;
    private View quitOnLeave;
    private com.google.android.material.materialswitch.MaterialSwitch quitOnLeaveSwitch;
    private TextView quitOnLeaveNote;
    private GameDetailModel quitOnLeaveGame;
    private ComputerDetails quitOnLeaveComputer;
    /** The backdrop, and the chip line with the installed and HDR chips. A layout without them (none today) leaves these null and shows the state chip alone. */
    private BackdropView backdrop;
    private View chips;
    private TextView installed;
    private TextView hdr;
    private TextView store;
    private ComputerDetails shownComputer;

    GameDetailView(GameDetailActivity activity) {
        this.activity = activity;
    }

    void bind(String hostUuid, int appId, String titleExtra, String hostNameExtra,
              Runnable onPlay, Runnable onEnd, Runnable onFavourite) {
        // The page heads itself with the game's title beside the cover; the bar keeps only the way back.
        if (titleExtra != null) activity.setTitle(titleExtra);
        TopAppBar.setUpNavigation(TopAppBar.configure(activity, R.id.detail_toolbar, "", 0),
                View.NO_ID, v -> activity.getOnBackPressedDispatcher().onBackPressed());
        // A game's page belongs to Home: Home in the bar is the page it came from, so it closes this one. The
        // other two replace Home's stack the way they do from Home itself.
        ShellBar.install(activity, ShellBar.Destination.HOME);
        activity.findViewById(R.id.shell_home).setOnClickListener(v -> activity.finish());
        loading = activity.findViewById(R.id.detail_loading);
        cover = activity.findViewById(R.id.detail_cover);
        coverText = activity.findViewById(R.id.detail_cover_text);
        title = activity.findViewById(R.id.detail_title);
        status = activity.findViewById(R.id.detail_status);
        host = activity.findViewById(R.id.detail_host);
        factsView = new TextView(activity);
        androidx.core.widget.TextViewCompat.setTextAppearance(factsView, R.style.TextAppearance_Nightjar_Supporting);
        factsView.setTextColor(host.getTextColors());
        factsView.setPadding(0, activity.getResources().getDimensionPixelSize(R.dimen.space_xs), 0, 0);
        factsView.setVisibility(View.GONE);
        ViewGroup hostParent = (ViewGroup) host.getParent();
        hostParent.addView(factsView, hostParent.indexOfChild(host) + 1);
        note = activity.findViewById(R.id.detail_note);
        play = activity.findViewById(R.id.detail_play);
        end = activity.findViewById(R.id.detail_end);
        favourite = activity.findViewById(R.id.detail_favourite);
        profileRow = activity.findViewById(R.id.detail_profile);
        profileValues = activity.findViewById(R.id.detail_profile_values);
        profileLayers = activity.findViewById(R.id.detail_profile_layers);
        profileRow.setOnClickListener(v -> chooseProfile());
        quitOnLeave = activity.findViewById(R.id.detail_quit_on_leave);
        quitOnLeaveSwitch = activity.findViewById(R.id.detail_quit_on_leave_switch);
        quitOnLeaveNote = activity.findViewById(R.id.detail_quit_on_leave_note);
        quitOnLeave.setOnClickListener(v -> onQuitOnLeave());
        androidx.core.view.ViewCompat.setAccessibilityDelegate(quitOnLeave, new androidx.core.view.AccessibilityDelegateCompat() {
            @Override
            public void onInitializeAccessibilityNodeInfo(View host,
                    androidx.core.view.accessibility.AccessibilityNodeInfoCompat info) {
                super.onInitializeAccessibilityNodeInfo(host, info);
                info.setClassName(android.widget.Switch.class.getName());
                info.setCheckable(true);
                info.setChecked(quitOnLeaveSwitch.isChecked());
            }
        });
        chips = activity.findViewById(R.id.detail_chips);
        installed = activity.findViewById(R.id.detail_installed);
        hdr = activity.findViewById(R.id.detail_hdr);
        store = activity.findViewById(R.id.detail_store);
        backdrop = activity.findViewById(R.id.detail_backdrop);
        if (backdrop != null) setUpAmbience(hostUuid, appId, titleExtra);
        play.setOnClickListener(v -> onPlay.run());
        end.setOnClickListener(v -> onEnd.run());
        favourite.setOnClickListener(v -> onFavourite.run());
        title.setText(titleExtra);
        host.setText(hostNameExtra);
        play.setEnabled(false);
        // The first D-pad press after a touch (or an adb start) lands on Play, not on the back arrow at the top.
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) play.setFocusedByDefault(true);
    }

    boolean hasBackdrop() {
        return backdrop != null;
    }

    void setLoading(boolean visible) {
        loading.setVisibility(visible ? View.VISIBLE : View.GONE);
    }

    /**
     * The page sits on the game's backdrop on every form. Opened from Home, it is the picture Home was showing for the game,
     * shown at once, so the page change keeps the screen's light; otherwise it fades in when the cover is loaded.
     */
    private void setUpAmbience(String hostUuid, int appId, String titleExtra) {
        com.limelight.ui.GlassForm form = com.limelight.ui.GlassForm.of(activity);
        boolean tv = form == com.limelight.ui.GlassForm.TELEVISION;
        int canvas = tv ? AppTheme.tvCanvas(activity)
                : com.google.android.material.color.MaterialColors.getColor(backdrop, R.attr.glassCanvas);
        // Held sideways the page is Home's hero grown to a page: the ambient bakes for the theme and fades as Home's does.
        // That layout is the sideways one whatever the device, so a phone turned on its side bakes as a handheld does.
        // Held upright the page is the sheet: the ambient is a phone Home's, the art at the top where the cover is.
        boolean sideways = activity.getResources().getConfiguration().orientation == android.content.res.Configuration.ORIENTATION_LANDSCAPE;
        if (!tv) HomeChrome.setUpBackdrop(activity, sideways ? com.limelight.ui.GlassForm.HANDHELD : com.limelight.ui.GlassForm.PHONE, backdrop, canvas);
        activity.getWindow().setBackgroundDrawable(new ColorDrawable(canvas));
        ((ViewGroup) activity.findViewById(android.R.id.content)).setClipChildren(false);
        // The system bars' padding sits on the content view as well: the backdrop reaches under the status bar, as Home's does.
        if (!tv) ((ViewGroup) activity.findViewById(android.R.id.content)).setClipToPadding(false);
        // Nothing on the page clips its children, so a focus halo and the sheet's foot can draw past their views. Scrolled,
        // the page would then run over the bar above it: the scrolling part is cut at its own top edge, and only there.
        View scroll = activity.findViewById(R.id.detail_scroll);
        if (scroll != null) {
            int far = activity.getResources().getDisplayMetrics().heightPixels;
            scroll.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) ->
                    v.setClipBounds(new android.graphics.Rect(-far, 0, r - l + far, b - t + far)));
        }
        backdrop.setCanvasColor(canvas);
        backdrop.setDim(AppTheme.isHighContrast(activity) ? 0.85f : 1f);
        backdrop.setOnCovered(() -> activity.getWindow().setBackgroundDrawable(null));
        backdrop.adopt(HomeAmbience.gameKey(hostUuid, appId, titleExtra));
    }

    /** Asks the backdrop for this game's cover; a cover still loading asks again when it arrives (see the art listener). */
    void askBackdrop() {
        GameDetailModel m = activity.model;
        if (backdrop == null || activity.covers == null || m == null) return;
        NvApp app = CoverLoaders.appOf(m.title, m.appId, m.source);
        ComputerDetails from = CoverLoaders.coverHost(shownComputer, m.source);
        backdrop.focus(HomeAmbience.gameKey(activity.hostUuid, activity.appId, activity.titleExtra),
                activity.covers.peek(from, app), activity.covers.lacksArt(from, app), m.title, false);
    }

    void render(GameDetailModel m, ComputerDetails computer) {
        title.setText(m.title);
        host.setText(m.hostName);
        activity.setTitle(m.title);
        activity.covers.bind(CoverLoaders.coverHost(computer, m.source), CoverLoaders.appOf(m.title, m.appId, m.source), cover, coverText);
        renderFacts(m);

        CharSequence chip = null;
        if (m.state == GameDetailModel.State.RUNNING) {
            chip = activity.getString(R.string.console_badge_running);
        } else if (m.state == GameDetailModel.State.SUSPENDED) {
            chip = activity.getString(R.string.console_badge_suspended);
        }
        status.setText(chip);
        status.setVisibility(chip == null ? View.GONE : View.VISIBLE);
        if (chips == null) {
            // As on the library cards: a running game's chip leads with the green dot.
            status.setCompoundDrawablesRelativeWithIntrinsicBounds(
                    m.state == GameDetailModel.State.RUNNING ? R.drawable.status_dot_ok : 0, 0, 0, 0);
        }
        if (chips != null) {
            shownComputer = computer;
            renderChips(m, chip != null);
            askBackdrop();
        }

        renderPlay(m);
        if (!m.hostOnline) {
            note.setText(activity.isWakeWaiting() ? activity.getString(R.string.console_wake_waiting, m.hostName)
                    : activity.getString(R.string.library_offline));
            note.setVisibility(View.VISIBLE);
        } else if (m.blockedNote() != 0) {
            // Play is off: say why ("Not installed on Living-room PC").
            note.setText(activity.getString(m.blockedNote(), m.hostName));
            note.setVisibility(View.VISIBLE);
        } else if (m.otherRunningTitle != null) {
            note.setText(activity.getString(R.string.console_detail_replaces, m.otherRunningTitle));
            note.setVisibility(View.VISIBLE);
        } else {
            note.setVisibility(View.GONE);
        }

        switch (m.endAction) {
            case CLOSE_GAME:
                end.setText(R.string.stream_close_game);
                end.setVisibility(View.VISIBLE);
                break;
            case QUIT:
                end.setText(R.string.applist_menu_quit);
                end.setVisibility(View.VISIBLE);
                break;
            default:
                end.setVisibility(View.GONE);
                break;
        }

        renderFavourite(activity.favouriteCache);

        StreamProfileSummary profile = StreamProfileSummary.of(activity, computer,
                m.source != null ? m.source.appUuid : null, m.appId);
        profileValues.setText(profile.values);
        profileLayers.setText(profile.layersOrMain(activity));
        // The profile is the player's to change here, as in the library's menu; without profiles the row only tells.
        boolean choosable = StreamProfilePicker.inUse(activity) != null;
        profileRow.setClickable(choosable);
        profileRow.setFocusable(choosable);
        renderQuitOnLeave(m, computer);

        if (activity.focusPending && play.isEnabled() && !play.isInTouchMode()) {
            activity.focusPending = false;
            play.requestFocus();
        }
    }

    /** Opens the profile chooser; a choice is the active profile, and the values above follow it. */
    private void chooseProfile() {
        ActionSheet picker = StreamProfilePicker.chooser(activity, activity::rerender);
        if (picker != null) {
            picker.show();
        }
    }

    /** The game's own quit-on-leave switch, under the conditions of the library's long-press menu. */
    private void renderQuitOnLeave(GameDetailModel m, ComputerDetails computer) {
        boolean offered = offersQuitOnLeave(m, computer);
        quitOnLeave.setVisibility(offered ? View.VISIBLE : View.GONE);
        quitOnLeaveGame = offered ? m : null;
        quitOnLeaveComputer = offered ? computer : null;
        if (!offered) return;
        boolean enabled = com.limelight.preferences.AppQuitOnLeave.isEnabled(activity, activity.hostUuid, m.source.appUuid, m.appId);
        quitOnLeaveSwitch.setChecked(enabled);
        quitOnLeaveNote.setText(enabled ? R.string.library_quit_on_leave_on : R.string.library_quit_on_leave_off);
    }

    /**
     * Only for a game the host has added and this device may start: without the launch permission every Disconnect would
     * ask the PC to quit and fail. It also needs the game's entry in the library, whose UUID is the key the library,
     * AppQuitOnLeave and the stream check on a host that reports one; a page without it would write the id-only key.
     */
    static boolean offersQuitOnLeave(GameDetailModel m, ComputerDetails computer) {
        return computer != null && computer.hasPermission(ComputerDetails.PERMISSION_LAUNCH)
                && m.appId > 0 && m.source != null && m.source.added;
    }

    /** Flips it in place, as the library's menu does; the note and a spoken line say what leaving now does. */
    private void onQuitOnLeave() {
        GameDetailModel m = quitOnLeaveGame;
        if (m == null) return;
        boolean enabled = !quitOnLeaveSwitch.isChecked();
        com.limelight.preferences.AppQuitOnLeave.setEnabled(activity, activity.hostUuid, m.source.appUuid, m.appId, enabled);
        renderQuitOnLeave(m, quitOnLeaveComputer);
        quitOnLeave.announceForAccessibility(quitOnLeaveNote.getText());
    }

    /** The game's facts under the host line, from a record kept on disk or a lookup that runs in the background. */
    private void renderFacts(GameDetailModel m) {
        if (factsView == null || !GameInfo.factsWanted(activity)) return;
        GameInfo.Game game = new GameInfo.Game(m.title, m.source == null ? null : m.source.gameId,
                m.source == null ? null : m.source.storeSource, m.source == null ? null : m.source.storeId);
        String id = game.name + "\n" + game.gameId + "\n" + game.storeSource + "\n" + game.storeId;
        if (id.equals(factsShownFor)) return;
        factsShownFor = id;
        factsView.setVisibility(View.GONE);
        java.util.function.Consumer<org.json.JSONObject> show = facts -> {
            if (activity.isFinishing() || activity.isDestroyed() || !id.equals(factsShownFor)) return;
            java.util.List<String> lines = GameInfo.describe(activity, facts);
            if (lines.isEmpty()) return;
            factsView.setText(android.text.TextUtils.join("\n", lines));
            factsView.setVisibility(View.VISIBLE);
        };
        GameInfo info = GameInfo.get(activity);
        Thread thread = new Thread(() -> {
            org.json.JSONObject kept = info.cachedFacts(game);
            if (kept != null) activity.runOnUiThread(() -> show.accept(kept));
            else info.facts(game, show);
        }, "game-info-facts");
        thread.setDaemon(true);
        thread.start();
    }

    /** The TV chip line: the state with its dot, installed or not when the host says, and HDR when the game has it. */
    private void renderChips(GameDetailModel m, boolean hasState) {
        int dot = m.state == GameDetailModel.State.RUNNING ? R.drawable.tv_dot_live : R.drawable.tv_dot_suspended;
        status.setCompoundDrawablesRelativeWithIntrinsicBounds(hasState ? dot : 0, 0, 0, 0);
        Boolean isInstalled = m.source != null ? m.source.installed : null;
        installed.setText(Boolean.FALSE.equals(isInstalled) ? R.string.console_chip_not_installed : R.string.console_detail_installed);
        installed.setVisibility(isInstalled == null ? View.GONE : View.VISIBLE);
        hdr.setVisibility(m.source != null && m.source.hdrSupported ? View.VISIBLE : View.GONE);
        if (store != null) renderStore(m.source);
        boolean any = hasState || isInstalled != null || hdr.getVisibility() == View.VISIBLE
                || (store != null && store.getVisibility() == View.VISIBLE);
        chips.setVisibility(any ? View.VISIBLE : View.GONE);
        chips.setContentDescription(chipsDescription((ViewGroup) chips));
    }

    /** The store chip (TV layout only): the store's glyph, at the chip text's size, beside its name; gone for a game no store owns. */
    private void renderStore(ConsoleSnapshot.Source source) {
        String key = source == null ? null : source.storeSource;
        String name = source == null ? null : Stores.name(key, source.storeName);
        store.setText(name);
        store.setVisibility(name == null ? View.GONE : View.VISIBLE);
        int glyph = Stores.glyph(key);
        Drawable icon = glyph == 0 ? null : ContextCompat.getDrawable(activity, glyph);
        if (icon != null) {
            icon = icon.mutate();
            icon.setTint(store.getCurrentTextColor());
            int size = Math.round(store.getTextSize());
            icon.setBounds(0, 0, size, size);
        }
        store.setCompoundDrawablesRelative(icon, null, null, null);
    }

    /** What a screen reader hears for the chip line, read as one stop: the shown chips' texts, in order. */
    static CharSequence chipsDescription(ViewGroup chips) {
        StringBuilder spoken = new StringBuilder();
        for (int i = 0; i < chips.getChildCount(); i++) {
            View child = chips.getChildAt(i);
            if (child.getVisibility() != View.VISIBLE || !(child instanceof TextView)) continue;
            CharSequence text = ((TextView) child).getText();
            if (text == null || text.length() == 0) continue;
            if (spoken.length() > 0) spoken.append(", ");
            spoken.append(text);
        }
        return spoken.length() == 0 ? null : spoken.toString();
    }

    /** Play, Resume, or for a PC that is away Wake PC and play (Waking PC while the wait lasts); never greyed out then. */
    void renderPlay(GameDetailModel m) {
        boolean waiting = activity.isWakeWaiting();
        if (activity.wakeable(m)) {
            play.setText(waiting ? R.string.wol_waking_pc : R.string.console_wake_and_play);
        } else {
            play.setText(m.resumes() ? R.string.applist_menu_resume : R.string.library_play);
        }
        play.setEnabled(m.canStart() || activity.wakeable(m));
    }

    void renderFavourite(boolean favouriteCache) {
        favourite.setText(favouriteCache ? R.string.console_favourite_remove : R.string.console_favourite_add);
        favourite.setIconResource(favouriteCache ? R.drawable.ic_star_filled : R.drawable.ic_star);
    }
}
