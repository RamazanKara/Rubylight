package com.limelight.ui;

import android.app.Presentation;
import android.content.Context;
import android.os.Bundle;
import android.view.Display;
import android.view.Gravity;
import android.view.View;
import android.view.WindowManager;
import android.widget.FrameLayout;

/** Video output on the selected display. Input stays in the activity on the phone/tablet. */
public final class StreamPresentation extends Presentation {
    private final StreamView videoView;
    public StreamPresentation(Context context, Display display, boolean texture) {
        super(context, display);
        videoView = new StreamView(getContext());
        videoView.initializeSurface(texture);
    }
    public StreamView videoView() { return videoView; }
    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN |
                WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON | WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE);
        getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_FULLSCREEN |
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY);
        FrameLayout root = new FrameLayout(getContext()); root.setBackgroundColor(android.graphics.Color.BLACK);
        root.addView(videoView, new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT, Gravity.CENTER));
        setContentView(root);
    }
}
