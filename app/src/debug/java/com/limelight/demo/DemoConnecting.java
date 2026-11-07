package com.limelight.demo;

import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.os.Bundle;

import androidx.appcompat.app.AppCompatActivity;

import com.limelight.R;
import com.limelight.ui.ConnectingScreen;

import java.io.IOException;
import java.io.InputStream;

/** Connecting, launching and a failed connection with no host: {@code --es state connecting|starting|conn-error}. */
public class DemoConnecting extends AppCompatActivity {
    private ConnectingScreen screen;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        setTheme(R.style.StreamTheme);
        super.onCreate(savedInstanceState);
        getWindow().getDecorView().setBackgroundColor(0xFF000000);
        String state = getIntent().getStringExtra("state");
        if ("conn-error".equals(state)) {
            screen = ConnectingScreen.failure(this, "Racing Game", "Living-room PC", cover(), getString(R.string.conn_error_title),
                    "Stream failed to start.\nClose protected videos or restart the PC.\n\nError code: 5031", new ConnectingScreen.Failure() {
                        @Override public void onTryAgain() {
                            finish();
                        }

                        @Override public CharSequence testNetwork() {
                            try {
                                Thread.sleep(1500);
                            } catch (InterruptedException ignored) {
                                Thread.currentThread().interrupt();
                            }
                            return getString(R.string.nettest_text_blocked);
                        }

                        @Override public void onBack() {
                            finish();
                        }
                    });
            screen.showFailure();
            return;
        }
        boolean starting = "starting".equals(state);
        screen = ConnectingScreen.show(this, starting ? "Big Picture" : "Racing Game", "Living-room PC",
                getString(R.string.conn_establishing_title), getString(R.string.conn_establishing_msg), this::finish);
        if (starting) {
            screen.showCover(null);
            screen.setStage(getString(R.string.conn_starting));
        } else {
            screen.showCover(cover());
        }
    }

    private Bitmap cover() {
        try (InputStream input = getAssets().open("demo/art/" + racingArt() + ".jpg")) {
            return BitmapFactory.decodeStream(input);
        } catch (IOException e) {
            return null;
        }
    }

    private static int racingArt() {
        for (int i = 0; i < DemoConsole.LIBRARY.length; i++) {
            if ("Racing Game".equals(DemoConsole.LIBRARY[i][0])) return i + 1;
        }
        return 1;
    }

    @Override
    protected void onDestroy() {
        if (screen != null) screen.dismiss();
        super.onDestroy();
    }
}
