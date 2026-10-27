package com.limelight.console;

import android.content.Context;

import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.PairingManager;

import java.util.Collections;
import java.util.HashSet;
import java.util.Set;

/**
 * Reads the library of a paired host with the native API once, so Home has something to open on the
 * next launch. The list of PCs calls it for each host it sees.
 */
public final class ConsoleSeed {
    private static final Set<String> TRIED = new HashSet<>();

    private ConsoleSeed() {
    }

    /** Safe to call often and from any thread; reads the library on a worker thread, at most once per host and run. */
    public static void seedOnce(Context context, String uniqueId, ComputerDetails computer) {
        seedOnce(context, uniqueId, computer, null);
    }

    /**
     * As above; {@code onSeeded} runs on the worker thread once a library exists on disk, so a screen can
     * show Home without waiting for its next resume.
     */
    public static void seedOnce(Context context, String uniqueId, ComputerDetails computer, Runnable onSeeded) {
        if (computer.pairState != PairingManager.PairState.PAIRED || computer.state != ComputerDetails.State.ONLINE
                || !computer.caps.has(HostCapabilities.Feature.NATIVE_API)) {
            return;
        }
        Context app = context.getApplicationContext();
        synchronized (TRIED) {
            if (ConsoleRepository.hasLibrary(app, computer.uuid) || !TRIED.add(computer.uuid)) {
                return;
            }
        }
        ComputerDetails snapshot = new ComputerDetails(computer);
        Thread thread = new Thread(() -> {
            try {
                ConsoleRepository.create(app, uniqueId).refresh(Collections.singletonList(snapshot));
                if (onSeeded != null && ConsoleRepository.hasAnyLibrary(app)) onSeeded.run();
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        }, "console-seed");
        thread.setDaemon(true);
        thread.start();
    }
}
