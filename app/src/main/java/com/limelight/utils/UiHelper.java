package com.limelight.utils;

import android.app.Activity;
import androidx.appcompat.app.AlertDialog;
import com.limelight.ui.NavigationDialogBuilder;
import android.app.GameManager;
import android.app.GameState;
import android.app.LocaleManager;
import android.content.Context;
import android.content.DialogInterface;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.graphics.Color;
import android.os.Build;
import android.os.LocaleList;
import android.view.View;
import android.view.WindowManager;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsControllerCompat;

import com.limelight.Game;
import com.limelight.R;
import com.limelight.ui.AdaptiveLayout;
import com.limelight.ui.UiNavigation;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.preferences.PreferenceConfiguration;

import java.util.Locale;

public class UiHelper {

    public static void showDialog(Context context, android.app.Dialog dialog) {
        // Native preferences no longer own the lifecycle of their Material dialogs.
        androidx.lifecycle.Lifecycle lifecycle = ((androidx.lifecycle.LifecycleOwner) context).getLifecycle();
        androidx.lifecycle.LifecycleEventObserver observer = (owner, event) -> {
            if (event == androidx.lifecycle.Lifecycle.Event.ON_DESTROY) dialog.dismiss();
        };
        dialog.setOnDismissListener(ignored -> lifecycle.removeObserver(observer));
        lifecycle.addObserver(observer);
        dialog.show();
        AdaptiveLayout.attach((Activity) context, dialog.findViewById(android.R.id.content));
    }

    private static void setGameModeStatus(Context context, boolean streaming, boolean interruptible) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            try {
                GameManager gameManager = context.getSystemService(GameManager.class);
                if (gameManager == null) {
                    return; // Not supported on this device (e.g., Meta Quest)
                }

                if (streaming) {
                    gameManager.setGameState(new GameState(false, interruptible ? GameState.MODE_GAMEPLAY_INTERRUPTIBLE : GameState.MODE_GAMEPLAY_UNINTERRUPTIBLE));
                } else {
                    gameManager.setGameState(new GameState(false, GameState.MODE_NONE));
                }
            } catch (Throwable t) {
                // Swallow any failure. Some OEM builds ship partial/incompatible GameManager impls.
            }
        }
    }

    public static void notifyStreamConnecting(Context context) {
        setGameModeStatus(context, true, true);
    }

    public static void notifyStreamConnected(Context context) {
        setGameModeStatus(context, true, false);
    }

    public static void notifyStreamEnteringPiP(Context context) {
        setGameModeStatus(context, true, true);
    }

    public static void notifyStreamExitingPiP(Context context) {
        setGameModeStatus(context, true, false);
    }

    public static void notifyStreamEnded(Context context) {
        setGameModeStatus(context, false, false);
    }

    public static void setLocale(Activity activity)
    {
        String locale = PreferenceConfiguration.readPreferences(activity).language;
        if (!locale.equals(PreferenceConfiguration.DEFAULT_LANGUAGE)) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                // On Android 13, migrate this non-default language setting into the OS native API
                LocaleManager localeManager = activity.getSystemService(LocaleManager.class);
                localeManager.setApplicationLocales(LocaleList.forLanguageTags(locale));
                PreferenceConfiguration.completeLanguagePreferenceMigration(activity);
            }
            else {
                Configuration config = new Configuration(activity.getResources().getConfiguration());

                // Some locales include both language and country which must be separated
                // before calling the Locale constructor.
                if (locale.contains("-"))
                {
                    config.locale = new Locale(locale.substring(0, locale.indexOf('-')),
                            locale.substring(locale.indexOf('-') + 1));
                }
                else
                {
                    config.locale = new Locale(locale);
                }

                activity.getResources().updateConfiguration(config, activity.getResources().getDisplayMetrics());
            }
        }
    }

    public static void notifyNewRootView(final Activity activity)
    {
        View rootView = activity.findViewById(android.R.id.content);

        // Set GameState.MODE_NONE initially for all activities
        setGameModeStatus(activity, false, false);

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            // Allow this non-streaming activity to layout under notches.
            //
            // We should NOT do this for the Game activity unless
            // the user specifically opts in, because it can obscure
            // parts of the streaming surface.
            activity.getWindow().getAttributes().layoutInDisplayCutoutMode =
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
        }

        WindowCompat.setDecorFitsSystemWindows(activity.getWindow(), false);
        activity.getWindow().clearFlags(WindowManager.LayoutParams.FLAG_TRANSLUCENT_STATUS |
                WindowManager.LayoutParams.FLAG_TRANSLUCENT_NAVIGATION);
        boolean oledBlack = com.limelight.ui.AppTheme.isOledBlack(activity);
        int fallbackBarColor = oledBlack ? Color.BLACK : Color.rgb(17, 26, 31);
        activity.getWindow().setStatusBarColor(Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ?
                Color.TRANSPARENT : fallbackBarColor);
        activity.getWindow().setNavigationBarColor(Build.VERSION.SDK_INT >= Build.VERSION_CODES.O ?
                Color.TRANSPARENT : fallbackBarColor);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            activity.getWindow().setStatusBarContrastEnforced(false);
            activity.getWindow().setNavigationBarContrastEnforced(false);
            rootView.setForceDarkAllowed(false);
        }
        boolean light = !oledBlack && activity.getResources().getBoolean(R.bool.light_system_bars);
        WindowInsetsControllerCompat controller = WindowCompat.getInsetsController(activity.getWindow(), rootView);
        controller.setAppearanceLightStatusBars(light);
        controller.setAppearanceLightNavigationBars(light);

        if (rootView.getTag(R.id.adaptive_layout) == null) UiNavigation.install(activity, rootView);
        AdaptiveLayout.attach(activity, rootView);
    }

    public static void showDecoderCrashDialog(Activity activity) {
        final SharedPreferences prefs = activity.getSharedPreferences("DecoderTombstone", 0);
        final int crashCount = prefs.getInt("CrashCount", 0);
        int lastNotifiedCrashCount = prefs.getInt("LastNotifiedCrashCount", 0);

        // Remember the last crash count we notified at, so we don't
        // display the crash dialog every time the app is started until
        // they stream again
        if (crashCount != 0 && crashCount != lastNotifiedCrashCount) {
            if (crashCount % 3 == 0) {
                // At 3 consecutive crashes, we'll forcefully reset their settings
                PreferenceConfiguration.resetStreamingSettings(activity);
                Dialog.displayDialog(activity,
                        activity.getResources().getString(R.string.title_decoding_reset),
                        activity.getResources().getString(R.string.message_decoding_reset),
                        new Runnable() {
                            @Override
                            public void run() {
                                // Mark notification as acknowledged on dismissal
                                prefs.edit().putInt("LastNotifiedCrashCount", crashCount).apply();
                            }
                        });
            }
            else {
                Dialog.displayDialog(activity,
                        activity.getResources().getString(R.string.title_decoding_error),
                        activity.getResources().getString(R.string.message_decoding_error),
                        new Runnable() {
                            @Override
                            public void run() {
                                // Mark notification as acknowledged on dismissal
                                prefs.edit().putInt("LastNotifiedCrashCount", crashCount).apply();
                            }
                        });
            }
        }
    }

    public static void displayQuitConfirmationDialog(Activity parent, final Runnable onYes, final Runnable onNo) {
        DialogInterface.OnClickListener dialogClickListener = new DialogInterface.OnClickListener() {
            @Override
            public void onClick(DialogInterface dialog, int which) {
                switch (which){
                    case DialogInterface.BUTTON_POSITIVE:
                        if (onYes != null) {
                            onYes.run();
                        }
                        break;

                    case DialogInterface.BUTTON_NEGATIVE:
                        if (onNo != null) {
                            onNo.run();
                        }
                        break;
                }
            }
        };

        AlertDialog.Builder builder = new NavigationDialogBuilder(parent);
        builder.setMessage(parent.getResources().getString(R.string.applist_quit_confirmation))
                .setPositiveButton(parent.getResources().getString(R.string.yes), dialogClickListener)
                .setNegativeButton(parent.getResources().getString(R.string.no), dialogClickListener)
                .setOnCancelListener(dialog -> { if (onNo != null) onNo.run(); })
                .show();
    }

    public static void displayDeletePcConfirmationDialog(Activity parent, ComputerDetails computer, final Runnable onYes, final Runnable onNo) {
        DialogInterface.OnClickListener dialogClickListener = new DialogInterface.OnClickListener() {
            @Override
            public void onClick(DialogInterface dialog, int which) {
                switch (which){
                    case DialogInterface.BUTTON_POSITIVE:
                        if (onYes != null) {
                            onYes.run();
                        }
                        break;

                    case DialogInterface.BUTTON_NEGATIVE:
                        if (onNo != null) {
                            onNo.run();
                        }
                        break;
                }
            }
        };

        AlertDialog.Builder builder = new NavigationDialogBuilder(parent);
        AlertDialog dialog = builder.setMessage(parent.getResources().getString(R.string.delete_pc_msg))
                .setTitle(computer.name)
                .setPositiveButton(parent.getResources().getString(R.string.pcview_menu_delete_pc), dialogClickListener)
                .setNegativeButton(android.R.string.cancel, dialogClickListener)
                .setOnCancelListener(ignored -> { if (onNo != null) onNo.run(); })
                .show();
        markDestructive(dialog);
    }

    /**
     * Gives a shown confirmation's positive button the destructive style (Widget.Nightjar.Button.Destructive):
     * the error colour for choices that remove or reset something.
     */
    public static void markDestructive(AlertDialog dialog) {
        android.widget.Button confirm = dialog.getButton(DialogInterface.BUTTON_POSITIVE);
        confirm.setTextColor(com.google.android.material.color.MaterialColors.getColor(
                dialog.getContext(), androidx.appcompat.R.attr.colorError, 0));
        if (confirm instanceof com.google.android.material.button.MaterialButton) {
            ((com.google.android.material.button.MaterialButton) confirm).setRippleColor(
                    androidx.core.content.ContextCompat.getColorStateList(dialog.getContext(), R.color.error_ripple));
        }
    }
}
