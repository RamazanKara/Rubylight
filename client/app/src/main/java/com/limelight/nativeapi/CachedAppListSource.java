package com.limelight.nativeapi;

import com.limelight.LimeLog;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.utils.CacheHelper;

import java.io.File;
import java.io.StringReader;
import java.util.List;

/**
 * {@link ClassicAppSource} from what the app list poller already keeps: the last raw list in
 * memory, else the copy in the cache directory that PcView and AppView read.
 */
public final class CachedAppListSource implements ClassicAppSource {
    private final File cacheDir;

    public CachedAppListSource(File cacheDir) {
        this.cacheDir = cacheDir;
    }

    @Override
    public List<NvApp> apps(ComputerDetails computer) {
        String raw = computer.rawAppList;
        try {
            if (raw == null) {
                if (!CacheHelper.cacheFileExists(cacheDir, "applist", computer.uuid)) {
                    return null;
                }
                raw = CacheHelper.readInputStreamToString(
                        CacheHelper.openCacheFileForInput(cacheDir, "applist", computer.uuid));
            }
            return NvHTTP.getAppListByReader(new StringReader(raw), computer.caps);
        } catch (Exception e) {
            LimeLog.warning("Unreadable app list of " + computer.name + ": " + e.getMessage());
            return null;
        }
    }
}
