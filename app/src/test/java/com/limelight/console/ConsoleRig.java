package com.limelight.console;

import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.FileLibraryCache;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nativeapi.RecentsStore;
import com.limelight.nvstream.http.NvApp;

import java.io.File;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** A repository over scripted hosts for the screen model tests: in-memory recents, a library cache in a folder. */
final class ConsoleRig {
    final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();
    final Map<String, List<NvApp>> classicApps = new HashMap<>();
    final List<RecentsStore.Recent> recents = new ArrayList<>();
    final ConsoleRepository repository;
    private final ExecutorService executor = Executors.newFixedThreadPool(4);

    ConsoleRig(File cacheDirectory) {
        RecentsStore store = new RecentsStore() {
            @Override public List<Recent> load() {
                return new ArrayList<>(recents);
            }

            @Override public void record(Recent recent) {
                recents.add(0, recent);
            }

            @Override public void forgetHost(String hostUuid) {
                recents.removeIf(recent -> recent.hostUuid.equals(hostUuid));
            }
        };
        repository = new ConsoleRepository(transport,
                computer -> HostEndpoint.of(computer, "unique", NativeApiFixtures.crypto()),
                new FileLibraryCache(cacheDirectory), store, computer -> classicApps.get(computer.uuid), executor,
                () -> 5000L);
    }

    void close() {
        executor.shutdownNow();
    }
}
