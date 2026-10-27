package com.limelight.console;

import android.content.Context;
import android.graphics.Bitmap;
import android.widget.ImageView;
import android.widget.TextView;

import com.limelight.grid.assets.CachedAppAssetLoader;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.grid.assets.MemoryAssetLoader;
import com.limelight.grid.assets.NetworkAssetLoader;
import com.limelight.nativeapi.ConsoleSnapshot;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import java.util.HashMap;
import java.util.Map;
import java.util.function.Consumer;

/**
 * Covers for Home and the game detail: the library grid's own box-art loader (memory, disk, then the
 * host's /appasset), one per host. A cover is cached under the app's id and the cover version the host
 * reported, so a new cover version loads a new image.
 */
final class CoverLoaders {
    /** The art is fetched about this wide; the loader scales it down to the view. */
    private static final int ART_WIDTH_PX = 300;

    private final Context context;
    private final String uniqueId;
    private final int coverWidthDp;
    private final Map<String, CachedAppAssetLoader> loaders = new HashMap<>();
    private final Map<String, String> addresses = new HashMap<>();
    private Consumer<NvApp> artListener;

    CoverLoaders(Context context, String uniqueId, int coverWidthDp) {
        this.context = context;
        this.uniqueId = uniqueId;
        this.coverWidthDp = coverWidthDp;
    }

    /** The art of the game; {@code source} is null for a game the host's library does not list. */
    static NvApp appOf(String title, int appId, ConsoleSnapshot.Source source) {
        NvApp app = new NvApp(title, appId, source != null && source.hdrSupported);
        if (source != null) {
            app.setAppUuid(source.appUuid);
            app.setArtVersion(source.coverVersion == null ? "" : source.coverVersion);
        }
        return app;
    }

    /** Shows the cover in {@code image}, or the title in {@code text} until there is one. */
    void bind(ComputerDetails host, NvApp app, ImageView image, TextView text) {
        if (host == null) {
            text.setText(app.getAppName());
            text.setVisibility(android.view.View.VISIBLE);
            image.setImageDrawable(null);
            return;
        }
        loader(host).populateImageView(app, image, text);
    }

    /** The cover if it is in memory now (never starts a load), else null. */
    Bitmap peek(ComputerDetails host, NvApp app) {
        return host == null ? null : loader(host).peekCover(app);
    }

    /** True once the game is known to have no artwork, so its backdrop can stay the canvas instead of waiting for one. */
    boolean lacksArt(ComputerDetails host, NvApp app) {
        return host == null || loader(host).hasNoArt(app);
    }

    /** Called on the main thread when a cover that was still loading has arrived (or turned out to be missing). */
    void setArtListener(Consumer<NvApp> listener) {
        artListener = listener;
        for (CachedAppAssetLoader loader : loaders.values()) loader.setArtListener(listener);
    }

    private CachedAppAssetLoader loader(ComputerDetails host) {
        String address = host.activeAddress == null ? "" : host.activeAddress.address + ":" + host.activeAddress.port;
        CachedAppAssetLoader loader = loaders.get(host.uuid);
        if (loader == null || !address.equals(addresses.get(host.uuid))) {
            if (loader != null) {
                loader.cancelForegroundLoads();
                loader.cancelBackgroundLoads();
            }
            int dpi = context.getResources().getDisplayMetrics().densityDpi;
            double divisor = Math.max(1.0, ART_WIDTH_PX / (coverWidthDp * (dpi / 160.0)));
            loader = new CachedAppAssetLoader(host, divisor,
                    new NetworkAssetLoader(context, uniqueId), new MemoryAssetLoader(), new DiskAssetLoader(context),
                    // Without artwork the card shows the title on the cover's own surface.
                    Bitmap.createBitmap(1, 1, Bitmap.Config.ARGB_8888));
            loader.setArtListener(artListener);
            loaders.put(host.uuid, loader);
            addresses.put(host.uuid, address);
        }
        return loader;
    }

    /** Stops loading and frees what the loaders hold; call when the screen goes away. */
    void release() {
        for (CachedAppAssetLoader loader : loaders.values()) {
            loader.cancelForegroundLoads();
            loader.cancelBackgroundLoads();
            loader.freeCacheMemory();
        }
        loaders.clear();
        addresses.clear();
    }
}
