package com.limelight.computers;

import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiClient.PowerReply;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nvstream.jni.NightjarHostsJni;
import com.limelight.nvstream.jni.NightjarPolicyJni;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.concurrent.Executor;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.LongSupplier;

/**
 * Sleep PC: asks the host for a token, lets the person confirm, and delegates token staleness
 * and spending to {@link NightjarHostsJni#hostFlowSleepOffer} / {@link NightjarHostsJni#hostFlowSleepSpend}.
 */
final class SleepFlow {
    /** A token this old when the person confirms is fetched again: it could expire on the way. */
    static final long TOKEN_REFRESH_MS = NightjarPolicyJni.hostSleepTokenRefreshMs();

    interface Client {
        /** The paired host's native API client; throws when it can't be reached that way. */
        NativeApiClient open() throws NativeApiException;
    }

    interface Ui {
        /** The host offered a token: ask the person and run {@code sleep} on yes; it acts only the first time. */
        void confirm(Runnable sleep);

        /** The host accepted (202) and is going to sleep. */
        void accepted();

        /** The host answered without a token: not supported, not allowed for this device, or busy. */
        void unavailable(PowerReply offer);

        /** The host refused: its status_message as sent (possibly empty) and the HTTP status. */
        void refused(String message, int httpStatus);

        /** The host could not be asked or its answer was unreadable. */
        void failed(NativeApiException error);
    }

    private final Client client;
    private final Ui ui;
    private final Executor worker;
    private final Executor main;
    private final LongSupplier clockMs;
    private final LongSupplier wallClockMs;

    SleepFlow(Client client, Ui ui, Executor worker, Executor main, LongSupplier clockMs) {
        this(client, ui, worker, main, clockMs, System::currentTimeMillis);
    }

    SleepFlow(Client client, Ui ui, Executor worker, Executor main, LongSupplier clockMs, LongSupplier wallClockMs) {
        this.client = client;
        this.ui = ui;
        this.worker = worker;
        this.main = main;
        this.clockMs = clockMs;
        this.wallClockMs = wallClockMs;
    }

    void start() {
        worker.execute(() -> {
            try {
                NativeApiClient api = client.open();
                long monoAsked = clockMs.getAsLong();
                long wallAsked = wallClockMs.getAsLong();
                JSONObject res = new JSONObject(NightjarHostsJni.hostFlowSleepOffer(api::requestRaw));
                main.execute(() -> deliverOffer(api, res, monoAsked, wallAsked));
            } catch (NativeApiException e) {
                main.execute(() -> fail(e));
            } catch (JSONException e) {
                main.execute(() -> ui.failed(new NativeApiException(NativeApiException.Kind.MALFORMED, e.getMessage())));
            }
        });
    }

    private void deliverOffer(NativeApiClient api, JSONObject res, long monoAsked, long wallAsked) {
        if ("token".equals(res.optString("outcome"))) {
            final String token = res.optString("token", "");
            AtomicBoolean confirmed = new AtomicBoolean();
            ui.confirm(() -> {
                if (confirmed.compareAndSet(false, true)) {
                    worker.execute(() -> spend(api, token, monoAsked, wallAsked));
                }
            });
        } else {
            deliverOutcome(res);
        }
    }

    private void spend(NativeApiClient api, String token, long monoAsked, long wallAsked) {
        try {
            String raw = NightjarHostsJni.hostFlowSleepSpend(
                    api::requestRaw, token, monoAsked, wallAsked, clockMs.getAsLong(), wallClockMs.getAsLong());
            JSONObject res = new JSONObject(raw);
            main.execute(() -> deliverOutcome(res));
        } catch (JSONException e) {
            main.execute(() -> ui.failed(new NativeApiException(NativeApiException.Kind.MALFORMED, e.getMessage())));
        }
    }

    private void deliverOutcome(JSONObject res) {
        switch (res.optString("outcome")) {
            case "accepted":
                ui.accepted();
                break;
            case "unavailable": {
                int reason = res.optInt("reason", 2);
                ui.unavailable(PowerReply.unavailable(reason != 0, reason == 2));
                break;
            }
            case "refused":
                ui.refused(res.optString("message", ""), res.optInt("status", 0));
                break;
            default:
                ui.failed(exceptionFromJson(res.optJSONObject("error")));
                break;
        }
    }

    private void fail(NativeApiException e) {
        if (e.getKind() == NativeApiException.Kind.POWER_REFUSED) {
            ui.refused(e.getMessage(), e.getHttpStatus());
        } else {
            ui.failed(e);
        }
    }

    static NativeApiException exceptionFromJson(JSONObject err) {
        if (err == null) {
            return new NativeApiException(NativeApiException.Kind.MALFORMED, "Unrecognized native API reply");
        }
        return new NativeApiException(
                NativeApiException.Kind.of(err.optString("kind")),
                err.optString("message", "Native API error"),
                err.optInt("http_status", 0),
                err.isNull("code") ? null : err.optString("code", null),
                err.isNull("setting") ? null : err.optString("setting", null),
                err.optLong("retry_after_ms", 0),
                err.isNull("title") ? null : err.optString("title", null),
                err.isNull("next_step") ? null : err.optString("next_step", null),
                err.isNull("technical_detail") ? null : err.optString("technical_detail", null));
    }
}
