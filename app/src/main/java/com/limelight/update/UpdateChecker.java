package com.limelight.update;

import com.limelight.BuildConfig;

import org.json.JSONException;

import java.io.IOException;
import java.util.concurrent.TimeUnit;

import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.Response;

/** Asks GitHub for the latest published release. Anonymous, so it is rate limited: callers stay quiet about failures. */
public final class UpdateChecker {
    static final String LATEST_RELEASE_URL = "https://api.github.com/repos/RamazanKara/nightjar/releases/latest";

    private static final OkHttpClient CLIENT = new OkHttpClient.Builder()
            .connectTimeout(10, TimeUnit.SECONDS)
            .readTimeout(15, TimeUnit.SECONDS)
            .callTimeout(30, TimeUnit.SECONDS)
            .build();

    private UpdateChecker() {
    }

    static OkHttpClient client() {
        return CLIENT;
    }

    static Request.Builder request(String url) {
        return new Request.Builder().url(url).header("User-Agent", "Nightjar/" + BuildConfig.VERSION_NAME + " (Android)");
    }

    /** Blocks on the network; call from a background thread. */
    public static ReleaseInfo fetchLatest() throws IOException {
        Request request = request(LATEST_RELEASE_URL)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .build();
        try (Response response = CLIENT.newCall(request).execute()) {
            if (response.code() == 403 || response.code() == 429) {
                throw new UpdateException.RateLimited();
            }
            if (!response.isSuccessful() || response.body() == null) {
                throw new IOException("GitHub answered " + response.code());
            }
            return ReleaseInfo.parse(response.body().string());
        } catch (JSONException e) {
            throw new IOException("Unreadable release", e);
        }
    }
}
