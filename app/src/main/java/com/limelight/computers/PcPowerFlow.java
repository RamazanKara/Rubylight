package com.limelight.computers;

import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePcPower;

import java.util.concurrent.Executor;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.LongSupplier;

/**
 * Restart PC or Shut down PC, as {@link SleepFlow}: ask the host (GET /power) for a token, let the person confirm,
 * ask again when the token is {@link SleepFlow#TOKEN_REFRESH_MS} old or older, then spend it once. Host calls run on
 * {@code worker} and every outcome reaches {@link Ui} on {@code main}; none is dropped.
 */
final class PcPowerFlow {
    interface Ui {
        /**
         * The host offered a token: ask the person and run {@code go} on yes; it acts only the first time.
         * {@code wakeFromOff} is false when the PC may not wake again once it is off.
         */
        void confirm(boolean wakeFromOff, Runnable go);

        /** The host took the request (202). */
        void accepted();

        /** The host can't do it now, or not for this device: its reason as sent, "" when it gave none. */
        void unavailable(String reason);

        /** The host refused the request: its words as sent. */
        void refused(String message);

        /** The host could not be asked or its answer was unreadable. */
        void failed(NativeApiException error);
    }

    private final NativePcPower.Action action;
    private final SleepFlow.Client client;
    private final Ui ui;
    private final Executor worker;
    private final Executor main;
    private final LongSupplier clockMs;

    PcPowerFlow(NativePcPower.Action action, SleepFlow.Client client, Ui ui, Executor worker, Executor main,
                LongSupplier clockMs) {
        this.action = action;
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
                NativePcPower.Offer offer = api.pcPower();
                main.execute(() -> offered(api, offer, asked));
            } catch (NativeApiException e) {
                main.execute(() -> fail(e));
            }
        });
    }

    private void offered(NativeApiClient api, NativePcPower.Offer offer, long asked) {
        String token = offer.tokenFor(action);
        if (token == null) {
            ui.unavailable(offer.reason(action));
            return;
        }
        // A double tap or a key repeat must not spend the token twice.
        AtomicBoolean confirmed = new AtomicBoolean();
        ui.confirm(offer.wakeFromOff, () -> {
            if (confirmed.compareAndSet(false, true)) {
                worker.execute(() -> spend(api, token, asked));
            }
        });
    }

    private void spend(NativeApiClient api, String token, long asked) {
        try {
            if (clockMs.getAsLong() - asked >= SleepFlow.TOKEN_REFRESH_MS) {
                NativePcPower.Offer fresh = api.pcPower();
                token = fresh.tokenFor(action);
                if (token == null) {
                    main.execute(() -> ui.unavailable(fresh.reason(action)));
                    return;
                }
            }
            api.powerPc(action, token);
            main.execute(ui::accepted);
        } catch (NativeApiException e) {
            main.execute(() -> fail(e));
        }
    }

    private void fail(NativeApiException e) {
        if (isRefusal(e)) {
            ui.refused(e.getMessage());
        } else {
            ui.failed(e);
        }
    }

    /** The host answered with an API error that carries its own words: shown as they are. */
    static boolean isRefusal(NativeApiException e) {
        return e.getKind() == NativeApiException.Kind.API && e.getMessage() != null && !e.getMessage().isEmpty();
    }
}
