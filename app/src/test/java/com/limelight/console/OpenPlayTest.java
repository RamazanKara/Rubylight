package com.limelight.console;

import android.app.Application;
import android.content.Context;
import android.content.SharedPreferences;
import android.content.Intent;
import android.preference.PreferenceManager;

import com.limelight.nativeapi.PreferencesRecentsStore;
import com.limelight.nativeapi.RecentsStore;
import com.limelight.utils.LaunchSource;

import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowToast;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/**
 * The automatic start's facts and guard on Android. The contract test holds {@link OpenPlayModel} to
 * protocol/android/src/open_play_contract.json, which the Rust test checks the JNI entry answers exactly.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class OpenPlayTest {
    private Context context;
    private final List<JSONObject> asked = new ArrayList<>();
    private String answer;

    @Before
    public void setUp() {
        context = org.robolectric.RuntimeEnvironment.getApplication();
        answer = "{\"ok\":{\"action\":\"stay\",\"game\":null,\"disarm\":false}}";
        OpenPlayModel.use(request -> {
            try {
                asked.add(new JSONObject(request));
            } catch (Exception e) {
                throw new AssertionError(e);
            }
            return answer;
        });
        new PreferencesRecentsStore(context).record(new RecentsStore.Recent("pc", "Desk", 7, "Dota 2", 1));
    }

    @After
    public void forgetTheModel() {
        OpenPlayModel.use(null);
    }

    private static Intent launcher() {
        return new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER);
    }

    private JSONObject lastAsked() {
        return asked.get(asked.size() - 1);
    }

    @Test
    public void armingIsOnDiskBeforeItReturns() {
        java.util.List<String> edits = new java.util.ArrayList<>();
        SharedPreferences real = context.getSharedPreferences("open_play", Context.MODE_PRIVATE);
        SharedPreferences.Editor editor = real.edit();
        java.lang.reflect.InvocationHandler editing = (self, method, args) -> {
            edits.add(method.getName());
            Object answer = method.invoke(editor, args);
            return answer == editor ? self : answer;
        };
        SharedPreferences watched = (SharedPreferences) java.lang.reflect.Proxy.newProxyInstance(getClass().getClassLoader(),
                new Class<?>[] {SharedPreferences.class}, (self, method, args) -> method.getName().equals("edit")
                        ? java.lang.reflect.Proxy.newProxyInstance(getClass().getClassLoader(),
                                new Class<?>[] {SharedPreferences.Editor.class}, editing)
                        : method.invoke(real, args));
        OpenPlay.arm(new android.content.ContextWrapper(context) {
            @Override public Context getApplicationContext() { return this; }
            @Override public SharedPreferences getSharedPreferences(String name, int mode) { return watched; }
        });
        assertTrue(edits.toString(), edits.contains("commit"));
        assertFalse(edits.toString(), edits.contains("apply"));
        assertTrue(OpenPlay.armed(context));
    }

    @Test
    public void theFactsOfEachWayInReachTheModel() throws Exception {
        PreferenceManager.getDefaultSharedPreferences(context).edit().putString(OpenPlay.SETTING, "last").apply();
        assertNull(OpenPlay.atEntry(context, launcher(), false, true));
        assertTrue(lastAsked().getBoolean("launcher"));
        assertTrue(lastAsked().getBoolean("first_screen"));
        assertTrue(lastAsked().getBoolean("has_last"));
        assertFalse(lastAsked().getBoolean("has_chosen"));
        assertEquals("last", lastAsked().getString("opens"));

        OpenPlay.atEntry(context, new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LEANBACK_LAUNCHER), false, true);
        assertTrue("a TV launcher counts", lastAsked().getBoolean("launcher"));

        OpenPlay.atEntry(context, new Intent(Intent.ACTION_MAIN), false, true);
        assertFalse("the list of PCs asked for by the app itself", lastAsked().getBoolean("launcher"));

        OpenPlay.atEntry(context, new Intent(Intent.ACTION_VIEW), false, true);
        assertFalse("a link", lastAsked().getBoolean("launcher"));

        OpenPlay.atEntry(context, launcher(), true, true);
        assertTrue("a recreated Activity", lastAsked().getBoolean("restored"));

        OpenPlay.atEntry(context, launcher(), false, false);
        assertFalse("a later screen of a running app", lastAsked().getBoolean("first_screen"));

        OpenPlay.atEntry(context, launcher().addFlags(Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY), false, true);
        assertTrue("Recents", lastAsked().getBoolean("from_recents"));
    }

    @Test
    public void aCountdownOpensTheGamesPageForTheGameTheModelNamed() {
        answer = "{\"ok\":{\"action\":\"countdown\",\"game\":\"last\",\"disarm\":false}}";
        Intent page = OpenPlay.atEntry(context, launcher(), false, true);
        assertEquals("pc", page.getStringExtra(GameDetailActivity.EXTRA_HOST_UUID));
        assertEquals(7, page.getIntExtra(GameDetailActivity.EXTRA_APP_ID, 0));
        assertEquals("Dota 2", page.getStringExtra(GameDetailActivity.EXTRA_TITLE));
        assertEquals("Desk", page.getStringExtra(GameDetailActivity.EXTRA_HOST_NAME));
        assertTrue(page.getBooleanExtra(GameDetailActivity.EXTRA_OPEN_PLAY, false));
    }

    @Test
    public void aSkipSaysSoOnceAndClearsTheGuard() throws Exception {
        OpenPlay.arm(context);
        answer = "{\"ok\":{\"action\":\"skip\",\"game\":\"last\",\"disarm\":true}}";
        assertNull(OpenPlay.atEntry(context, launcher(), false, true));
        assertTrue("armed reaches the model", lastAsked().getBoolean("armed"));
        assertFalse(OpenPlay.armed(context));
        assertTrue(ShadowToast.getTextOfLatestToast(), ShadowToast.getTextOfLatestToast().contains("Dota 2"));
    }

    @Test
    public void aModelThatCannotAnswerStartsNothing() {
        OpenPlay.arm(context);
        OpenPlayModel.use(request -> { throw new UnsatisfiedLinkError("no call"); });
        assertNull(OpenPlay.atEntry(context, launcher(), false, true));
        assertTrue("and the guard stands", OpenPlay.armed(context));
    }

    @Test
    public void aStreamEndClearsTheGuardOnlyWhenTheModelSays() {
        OpenPlay.arm(context);
        answer = "{\"ok\":{\"disarm\":false}}";
        OpenPlay.streamEnded(context, 4000, false);
        assertTrue(OpenPlay.armed(context));
        answer = "{\"ok\":{\"disarm\":true}}";
        OpenPlay.streamEnded(context, 4000, true);
        assertFalse(OpenPlay.armed(context));
    }

    @Test
    public void aStreamsSourceSaysItWasAutomaticAlsoAfterAReconnect() {
        Intent start = LaunchSource.put(new Intent(), LaunchSource.OPEN_PLAY);
        assertTrue(OpenPlay.isAutoStart(start));
        assertTrue(OpenPlay.isAutoStart(LaunchSource.put(new Intent(), LaunchSource.again(LaunchSource.RECONNECT, start))));
        assertFalse(OpenPlay.isAutoStart(LaunchSource.put(new Intent(), LaunchSource.SHORTCUT)));
        assertFalse(OpenPlay.isAutoStart(new Intent()));
    }

    @Test
    public void theMenuChoiceSetsTheSettingAndTakingItBackReturnsToThePcs() throws Exception {
        android.content.SharedPreferences settings = PreferenceManager.getDefaultSharedPreferences(context);
        String choose = OpenPlay.menuLabel(context, "pc", 9);
        OpenPlay.toggle(context, "pc", "Desk", 9, "t-hades", "Hades");
        assertEquals("chosen", settings.getString(OpenPlay.SETTING, null));
        assertNotEquals(choose, OpenPlay.menuLabel(context, "pc", 9));
        assertEquals("another game still offers it", choose, OpenPlay.menuLabel(context, "pc", 10));

        answer = "{\"ok\":{\"action\":\"countdown\",\"game\":\"chosen\",\"disarm\":false}}";
        assertEquals(9, OpenPlay.atEntry(context, launcher(), false, true).getIntExtra(GameDetailActivity.EXTRA_APP_ID, 0));
        assertTrue(lastAsked().getBoolean("has_chosen"));

        OpenPlay.toggle(context, "pc", "Desk", 9, "t-hades", "Hades");
        assertEquals("pcs", settings.getString(OpenPlay.SETTING, null));
        assertEquals(choose, OpenPlay.menuLabel(context, "pc", 9));
    }

    @Test
    public void aChosenGameCarriesItsStableIdAndAnOlderSaveOpensWithout() throws Exception {
        answer = "{\"ok\":{\"action\":\"countdown\",\"game\":\"chosen\",\"disarm\":false}}";
        OpenPlay.toggle(context, "pc", "Desk", 9, "t-hades", "Hades");
        assertEquals("t-hades", OpenPlay.atEntry(context, launcher(), false, true).getStringExtra(GameDetailActivity.EXTRA_STABLE_ID));
        context.getSharedPreferences("open_play", android.content.Context.MODE_PRIVATE).edit()
                .putString("game", "{\"host\":\"pc\",\"host_name\":\"Desk\",\"app\":9,\"title\":\"Hades\"}").commit();
        assertEquals("", OpenPlay.atEntry(context, launcher(), false, true).getStringExtra(GameDetailActivity.EXTRA_STABLE_ID));
    }

    @Test
    public void theLastGameCarriesItsStableIdAndAnOlderEntryOpensWithout() throws Exception {
        answer = "{\"ok\":{\"action\":\"countdown\",\"game\":\"last\",\"disarm\":false}}";
        assertEquals("older entry", "", OpenPlay.atEntry(context, launcher(), false, true).getStringExtra(GameDetailActivity.EXTRA_STABLE_ID));
        new PreferencesRecentsStore(context).record(new RecentsStore.Recent("pc", "Desk", 8, "Hades", 2, "t-hades"));
        assertEquals("t-hades", OpenPlay.atEntry(context, launcher(), false, true).getStringExtra(GameDetailActivity.EXTRA_STABLE_ID));
    }

    @Test
    public void removingThePcForgetsItsChosenGameOnly() {
        OpenPlay.toggle(context, "pc", "Desk", 9, "t-hades", "Hades");
        OpenPlay.forgetHost(context, "other");
        assertNotEquals(OpenPlay.menuLabel(context, "other", 9), OpenPlay.menuLabel(context, "pc", 9));
        OpenPlay.forgetHost(context, "pc");
        assertEquals(OpenPlay.menuLabel(context, "other", 9), OpenPlay.menuLabel(context, "pc", 9));
    }

    private static JSONArray contract() throws Exception {
        for (String path : new String[] {"../protocol/android/src/open_play_contract.json", "protocol/android/src/open_play_contract.json"}) {
            File file = new File(path);
            if (file.isFile()) return new JSONArray(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
        }
        throw new AssertionError("open_play_contract.json not found from " + new File(".").getAbsolutePath());
    }

    /** Each decide request the model sends is the shared contract's, and it reads the answer there as meant. */
    @Test
    public void theModelSpeaksTheSharedContract() throws Exception {
        JSONArray cases = contract();
        for (int i = 0; i < cases.length(); i++) {
            JSONObject c = cases.getJSONObject(i);
            JSONObject request = c.getJSONObject("request");
            JSONObject ok = c.getJSONObject("response").getJSONObject("ok");
            OpenPlayModel.use(sent -> {
                try {
                    assertEquals(c.getString("name"), request.toString(), sortedLike(request, new JSONObject(sent)).toString());
                    return c.getJSONObject("response").toString();
                } catch (Exception e) {
                    throw new AssertionError(e);
                }
            });
            switch (request.getString("op")) {
                case "decide":
                    OpenPlayModel.Decision d = OpenPlayModel.decide(request.getBoolean("first_screen"), request.getBoolean("launcher"),
                            request.getBoolean("restored"), request.getBoolean("from_recents"), request.getString("opens"),
                            request.getBoolean("has_last"), request.getBoolean("has_chosen"), request.getBoolean("armed"));
                    assertEquals(ok.getString("action"), d.action);
                    assertEquals(ok.isNull("game") ? null : ok.getString("game"), d.game);
                    assertEquals(ok.getBoolean("disarm"), d.disarm);
                    break;
                case "stream_end":
                    assertEquals(ok.getBoolean("disarm"), OpenPlayModel.streamEndDisarms(request.getLong("connected_ms"), request.getBoolean("left_by_player")));
                    break;
                case "timing":
                    assertArrayEquals(new long[] {ok.getLong("countdown_ms"), ok.getLong("settled_ms")}, OpenPlayModel.timing());
                    break;
                default:
                    fail("unknown op " + request);
            }
        }
    }

    /** The JSON of {@code sent} in the key order of {@code like}, failing when the keys differ. */
    private static JSONObject sortedLike(JSONObject like, JSONObject sent) throws Exception {
        assertEquals(like.length(), sent.length());
        JSONObject sorted = new JSONObject();
        for (java.util.Iterator<String> keys = like.keys(); keys.hasNext();) {
            String key = keys.next();
            sorted.put(key, sent.get(key));
        }
        return sorted;
    }
}
