package com.limelight.console;

import static com.limelight.console.RadeonFixtures.CHILL;
import static com.limelight.console.RadeonFixtures.CHILL_MAX;
import static com.limelight.console.RadeonFixtures.FRAME_TARGET;
import static com.limelight.console.RadeonFixtures.VSYNC;
import static com.limelight.console.RadeonFixtures.model;
import static com.limelight.console.RadeonFixtures.panel;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiException;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class RadeonPanelModelTest {
    private static RadeonPanelModel.Row row(RadeonPanelModel model, String id) {
        RadeonPanelModel.Row row = model.find(id);
        assertTrue(id, row != null);
        return row;
    }

    @Test
    public void configurableAndWritableNowMakesEditableControls() {
        RadeonPanelModel model = model(RadeonFixtures.configurable());
        assertEquals(RadeonPanelModel.Notice.NONE, model.notice);
        assertEquals(RadeonPanelModel.Mode.EDIT, row(model, "chill.enabled").mode);
        assertEquals(RadeonPanelModel.Mode.EDIT, row(model, "chill.max_fps").mode);
        assertEquals(RadeonPanelModel.Mode.EDIT, row(model, "vsync.mode").mode);
    }

    @Test
    public void configurableButNotWritableNowShowsValuesAndSaysWhy() {
        RadeonPanelModel noControl = model(panel("configurable", false, false, null, CHILL, CHILL_MAX));
        assertEquals(RadeonPanelModel.Notice.NEEDS_HOST_CONTROL, noControl.notice);
        assertEquals(RadeonPanelModel.Mode.VALUE, row(noControl, "chill.enabled").mode);
        assertEquals(RadeonPanelModel.Mode.VALUE, row(noControl, "chill.max_fps").mode);

        RadeonPanelModel noSession = model(panel("configurable", true, false, null, CHILL));
        assertEquals(RadeonPanelModel.Notice.NEEDS_SESSION, noSession.notice);
        assertEquals(RadeonPanelModel.Mode.VALUE, row(noSession, "chill.enabled").mode);
    }

    @Test
    public void readOnlyShowsValuesWithoutControls() {
        RadeonPanelModel model = model(panel("read_only", true, false, null,
                CHILL.replace("\"configurable\"", "\"read_only\"")));
        assertEquals(RadeonPanelModel.Notice.READ_ONLY, model.notice);
        assertEquals(RadeonPanelModel.Mode.VALUE, row(model, "chill.enabled").mode);
    }

    @Test
    public void needsExternalSetupKeepsTheHostMessageOnTheSetting() {
        RadeonPanelModel model = model(panel("needs_external_setup", true, false, null,
                "{\"id\":\"afmf.enabled\",\"group\":\"afmf\",\"kind\":\"toggle\",\"state\":\"needs_external_setup\","
                        + "\"value\":false,\"reason\":\"external_setup\",\"message\":\"Turn it on once in AMD Software.\"}"));
        assertEquals(RadeonPanelModel.Notice.NEEDS_EXTERNAL_SETUP, model.notice);
        RadeonPanelModel.Row afmf = row(model, "afmf.enabled");
        assertEquals(RadeonPanelModel.Mode.EXTERNAL_SETUP, afmf.mode);
        assertEquals("Turn it on once in AMD Software.", afmf.message);
    }

    @Test
    public void unsupportedGreysEverySettingOut() {
        RadeonPanelModel model = model(panel("unsupported", true, false, null,
                "{\"id\":\"rsr.enabled\",\"group\":\"rsr\",\"kind\":\"toggle\",\"state\":\"unsupported\"}"));
        assertEquals(RadeonPanelModel.Notice.UNSUPPORTED, model.notice);
        assertEquals(RadeonPanelModel.Mode.UNSUPPORTED, row(model, "rsr.enabled").mode);
    }

    @Test
    public void temporarilyUnavailableIsNotUnsupportedAndAdlxFailureHasNoRows() {
        RadeonPanelModel busy = model(panel("temporarily_unavailable", true, false,
                "\"reason\":\"driver_busy\",\"message\":\"The driver is busy.\"",
                "{\"id\":\"chill.enabled\",\"group\":\"chill\",\"kind\":\"toggle\",\"state\":\"temporarily_unavailable\","
                        + "\"reason\":\"driver_busy\",\"message\":\"The driver is busy. Try again in a moment.\"}"));
        assertEquals(RadeonPanelModel.Notice.UNAVAILABLE, busy.notice);
        assertEquals("The driver is busy.", busy.message);
        assertEquals(RadeonPanelModel.Mode.UNAVAILABLE, row(busy, "chill.enabled").mode);

        RadeonPanelModel adlx = model("{\"revision\":1,\"state\":\"temporarily_unavailable\",\"reason\":\"adlx_unavailable\","
                + "\"host_control\":true,\"writable_now\":false,\"gpus\":[]}");
        assertEquals(RadeonPanelModel.Notice.ADLX_UNAVAILABLE, adlx.notice);
        assertFalse(adlx.hasRows());
    }

    @Test
    public void aSettingOfAStateTheAppDoesNotKnowIsDrawnAsUnsupported() {
        RadeonPanelModel model = model(panel("configurable", true, true, null,
                "{\"id\":\"x.enabled\",\"group\":\"x\",\"kind\":\"toggle\",\"state\":\"from_the_future\"}"));
        assertEquals(RadeonPanelModel.Mode.UNSUPPORTED, row(model, "x.enabled").mode);
        assertEquals(RadeonPanelModel.Notice.UNSUPPORTED,
                model("{\"revision\":1,\"state\":\"from_the_future\",\"gpus\":[]}").notice);
    }

    @Test
    public void aSliderWithNoRoomToMoveIsShownAsAValue() {
        RadeonPanelModel model = model(panel("configurable", true, true, null,
                CHILL_MAX.replace("\"min\":30,\"max\":240", "\"min\":144,\"max\":144")));
        assertEquals(RadeonPanelModel.Mode.VALUE, row(model, "chill.max_fps").mode);
    }

    @Test
    public void aKindTheAppDoesNotKnowIsShownAsAValueNotAControl() {
        RadeonPanelModel model = model(panel("configurable", true, true, null,
                "{\"id\":\"y.thing\",\"group\":\"y\",\"kind\":\"colour\",\"state\":\"configurable\",\"value\":\"red\"}"));
        assertEquals(RadeonPanelModel.Mode.VALUE, row(model, "y.thing").mode);
    }

    @Test
    public void unknownIdsAreNamedFromTheirWordsAndKnownOnesFromStrings() {
        RadeonPanelModel model = model(panel("configurable", true, true, null,
                "{\"id\":\"deep_colour.max_bits\",\"group\":\"deep_colour\",\"kind\":\"integer\","
                        + "\"state\":\"configurable\",\"value\":10,\"range\":{\"min\":8,\"max\":12,\"step\":1},\"unit\":\"bit\"}",
                CHILL_MAX));
        RadeonPanelModel.Row unknown = row(model, "deep_colour.max_bits");
        assertEquals(0, unknown.groupTitleRes());
        assertEquals("Deep colour: Max bits", unknown.genericTitle());
        assertEquals("10 bit", unknown.valueText());
        RadeonPanelModel.Row known = row(model, "chill.max_fps");
        assertEquals(R.string.native_group_chill, known.groupTitleRes());
        assertEquals(R.string.native_field_max_fps, known.fieldTitleRes());
    }

    @Test
    public void percentHasNoSpaceAndOtherUnitsDo() {
        assertEquals("70%", RadeonPanelModel.withUnit("70", "%"));
        assertEquals("144 fps", RadeonPanelModel.withUnit("144", "fps"));
        assertEquals("144", RadeonPanelModel.withUnit("144", null));
    }

    @Test
    public void aChoiceShowsItsLabelAndAToggleShowsNoValueText() {
        RadeonPanelModel model = model(RadeonFixtures.configurable());
        assertEquals("Off", row(model, "vsync.mode").valueText());
        assertNull(row(model, "chill.enabled").valueText());
    }

    @Test
    public void theRestoreHintNamesTheOriginalWithItsUnit() {
        RadeonPanelModel model = model(RadeonFixtures.configurable());
        RadeonPanelModel.Row target = row(model, "frame_rate_target.fps");
        assertEquals("120 fps", RadeonPanelModel.restoreText(target, target.restore, "On", "Off"));
        assertNull(row(model, "chill.enabled").restore);
    }

    @Test
    public void integerWritesSnapIntoTheRangeAndItsStep() {
        RadeonPanelModel model = model(RadeonFixtures.configurable());
        RadeonPanelModel.Row max = row(model, "chill.max_fps");
        assertEquals(150L, RadeonPanelModel.writableValue(max, 148L));
        assertEquals(240L, RadeonPanelModel.writableValue(max, 999L));
        assertEquals(30L, RadeonPanelModel.writableValue(max, 3L));
        assertEquals(true, RadeonPanelModel.writableValue(row(model, "chill.enabled"), true));
        assertEquals("always", RadeonPanelModel.writableValue(row(model, "vsync.mode"), "always"));
    }

    @Test
    public void sameAsIgnoresTheRevisionButNotAValue() {
        RadeonPanelModel a = model(RadeonFixtures.configurable());
        RadeonPanelModel b = model(RadeonFixtures.configurable().replace("\"revision\":3", "\"revision\":4"));
        RadeonPanelModel c = model(RadeonFixtures.configurable().replace("\"value\":144", "\"value\":120"));
        assertTrue(a.sameAs(b));
        assertFalse(a.sameAs(c));
        assertFalse(a.sameAs(null));
    }

    private static NativeApiException api(int status, String code, String setting) {
        return new NativeApiException(NativeApiException.Kind.API, "host text", status, code, setting, 0);
    }

    @Test
    public void everyWriteErrorHasItsOwnMessageAndNamesTheSetting() {
        assertEquals(R.string.native_error_invalid_value,
                RadeonPanelModel.Failure.of(api(422, "invalid_value", "chill.max_fps"), "x").messageRes);
        assertEquals(R.string.native_error_not_writable,
                RadeonPanelModel.Failure.of(api(422, "not_writable", "rsr.enabled"), null).messageRes);
        RadeonPanelModel.Failure rejected = RadeonPanelModel.Failure.of(api(409, "driver_rejected", "chill.max_fps"), "other");
        assertEquals(R.string.native_error_driver_rejected, rejected.messageRes);
        assertEquals("the host names the setting", "chill.max_fps", rejected.settingId);
        assertEquals(R.string.native_error_host_control,
                RadeonPanelModel.Failure.of(api(403, "host_control_required", null), "chill.enabled").messageRes);
        assertEquals(R.string.native_error_host_control,
                RadeonPanelModel.Failure.of(api(403, "not_host_control", null), "chill.enabled").messageRes);
        assertEquals(R.string.native_error_no_session,
                RadeonPanelModel.Failure.of(api(409, "no_session", null), null).messageRes);
        assertEquals(R.string.native_error_no_session,
                RadeonPanelModel.Failure.of(api(409, "not_session_owner", null), null).messageRes);
        assertEquals(R.string.native_error_busy,
                RadeonPanelModel.Failure.of(api(409, "session_busy", null), null).messageRes);
        assertEquals(R.string.native_error_busy,
                RadeonPanelModel.Failure.of(api(503, "temporarily_unavailable", null), null).messageRes);
        assertEquals(R.string.native_error_busy,
                RadeonPanelModel.Failure.of(api(503, "something_new", null), null).messageRes);
    }

    @Test
    public void aFailureWithoutAKnownCodeKeepsTheHostTextAndTheRequestedSetting() {
        RadeonPanelModel.Failure failure = RadeonPanelModel.Failure.of(api(500, "something_new", null), "chill.enabled");
        assertEquals(R.string.native_error_generic, failure.messageRes);
        assertEquals("chill.enabled", failure.settingId);
        assertEquals("host text", failure.hostMessage);
        assertEquals(R.string.native_error_unreachable, RadeonPanelModel.Failure.of(
                new NativeApiException(NativeApiException.Kind.NETWORK, "refused"), "chill.enabled").messageRes);
    }

    @Test
    public void a404OrNotFoundMeansTheHostHasNoRadeonEndpoints() {
        assertTrue(RadeonPanelModel.isMissing(new NativeApiException(NativeApiException.Kind.NOT_FOUND, "no")));
        assertTrue(RadeonPanelModel.isMissing(
                new NativeApiException(NativeApiException.Kind.STATUS, "no", 404, null, null, 0)));
        assertFalse(RadeonPanelModel.isMissing(api(403, "host_control_required", null)));
    }
}
