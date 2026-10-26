package com.limelight.computers;

import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiClient.PowerReply;
import com.limelight.nativeapi.NativeApiException;

import java.util.concurrent.Executor;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.LongSupplier;

/**
 * Sleep PC: ask the host for a token, let the person confirm, ask again when the token is
 * {@link #TOKEN_REFRESH_MS} old or older (the host keeps it 30 s), then spend it once. Host calls run on {@code worker} and
 * every outcome reaches {@link Ui} on {@code main}; none is dropped.
 */
final class SleepFlow {
    /** A token this old when the person confirms is fetched again: it could expire on the way. */
    static final long TOKEN_REFRESH_MS = 25_000;

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

    SleepFlow(Client client, Ui ui, Executor worker, Executor main, LongSupplier clockMs) {
        this.client = client;
        this.ui = ui;
        this.worker = worker;
        this.main = main;
        this.clockMs = clockMs;
    }

    void start() {
        worker.execute(() -> {
            try {
                NativeApiClient api = client.open();
                // Timed from before the request: the host may have minted the token at any point during it.
                long asked = clockMs.getAsLong();
                PowerReply offer = api.powerCapabilities();
                main.execute(() -> offered(api, offer, asked));
            } catch (NativeApiException e) {
                main.execute(() -> fail(e));
            }
        });
    }

    private void offered(NativeApiClient api, PowerReply offer, long asked) {
        if (offer.token == null) {
            ui.unavailable(offer);
            return;
        }
        // A double tap or a key repeat must not spend the token twice.
        AtomicBoolean confirmed = new AtomicBoolean();
        ui.confirm(() -> {
            if (confirmed.compareAndSet(false, true)) {
                worker.execute(() -> sleep(api, offer.token, asked));
            }
        });
    }

    private void sleep(NativeApiClient api, String token, long asked) {
        try {
            if (clockMs.getAsLong() - asked >= TOKEN_REFRESH_MS) {
                PowerReply fresh = api.powerCapabilities();
                if (fresh.token == null) {
                    main.execute(() -> ui.unavailable(fresh));
                    return;
                }
                token = fresh.token;
            }
            PowerReply reply = api.sleep(token);
            main.execute(() -> {
                if (reply.accepted) {
                    ui.accepted();
                } else {
                    ui.refused(reply.message, reply.status);
                }
            });
        } catch (NativeApiException e) {
            main.execute(() -> fail(e));
        }
    }

    private void fail(NativeApiException e) {
        if (e.getKind() == NativeApiException.Kind.POWER_REFUSED) {
            ui.refused(e.getMessage(), e.getHttpStatus());
        } else {
            ui.failed(e);
        }
    }
}
