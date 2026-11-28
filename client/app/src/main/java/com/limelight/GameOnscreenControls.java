package com.limelight;

import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.TouchLayout;
import com.limelight.binding.input.pad.ModernController;
import com.limelight.binding.input.pad.PadVisibility;

/**
 * On-screen modern controller creation, visibility policy, physical-controller auto-hide,
 * game-button editing coordination, and pointer touchpad wiring extracted from {@link Game}.
 */
final class GameOnscreenControls {
    private final Game game;
    private final int[] pointerMoveDelta = new int[2];

    GameOnscreenControls(Game game) {
        this.game = game;
    }

    // The on-screen controls' switch, Hide while a controller is connected, and whether one is here. They also step
    // aside while the game buttons are moved: they lie above those buttons and would take the drags.
    boolean onscreenControlsShown() {
        return game.prefConfig != null && PadVisibility.shouldShow(
                game.prefConfig.onscreenController, game.prefConfig.oscAutoHideOnController, game.physicalControllerConnected,
                game.prefConfig.sendControllers, touchControlsWithPad() || controllerBringsLayout()) && !movingGameButtons();
    }

    /** A connected controller has an on-screen layout of its own (#154): the player keeps touch beside that pad. */
    private boolean controllerBringsLayout() {
        return game.physicalControllerConnected && ModernController.controllerBringsLayout(game, game.controllerModels);
    }

    /**
     * The connected controllers changed: the on-screen controls show the layout saved for one of them, if any, and
     * whether a controller hides them is decided again once one with its own layout came or went. Without such a
     * layout nothing else happens, so the controls show and hide exactly as before.
     */
    void setControllerModels(java.util.List<String[]> models) {
        boolean before = controllerBringsLayout();
        game.controllerModels = models;
        if (game.modernController != null) game.modernController.setControllers(models);
        if (controllerBringsLayout() != before) setPhysicalControllerConnected(game.physicalControllerConnected);
    }

    private boolean movingGameButtons() {
        return game.gameTouchControls != null && game.gameTouchControls.isEditing();
    }

    /**
     * The settings that deliberately use the touch controls together with a physical pad: Only L3/R3 supplements the
     * pad, and, in the one-player-each mode, the on-screen controls can be a player of their own beside it. A pad must
     * not hide them then.
     */
    private boolean touchControlsWithPad() {
        return game.prefConfig.onlyL3R3 || (game.prefConfig.onscreenOwnPlayer && game.prefConfig.multiController);
    }

    /** A physical controller came or went: the on-screen controls follow the Hide while a controller is connected setting. */
    void setPhysicalControllerConnected(boolean connected) {
        game.physicalControllerConnected = connected;
        if (game.prefConfig == null || !game.prefConfig.oscAutoHideOnController || !game.prefConfig.onscreenController) return;
        if (game.isFinishing()) return;
        // An open editor is the player's work; it stays until Done or Cancel, even with a controller in hand.
        if (connected && game.modernController != null && game.modernController.isEditing()) return;
        // The settings decide, not a pad's arrival: Only L3/R3, the on-screen controls as their own player, and
        // Controllers Off all keep touch with a pad, and this is where their controls used to disappear.
        if (!game.onscreenControlsShown()) {
            game.hideOnscreenControllers();
        } else if (!game.isHidingOverlays && game.conn != null && game.conn.canSendInput()) {
            game.showOnscreenController();
        }
    }

    /**
     * Moving the game buttons started or ended: the on-screen controls leave while it lasts and come back after, if the
     * settings show them. An open editor of the modern controls stays either way; the controls show once it closes.
     */
    void onGameButtonsEditing() {
        if (game.prefConfig == null || !game.prefConfig.onscreenController || game.isFinishing()) return;
        if (movingGameButtons()) {
            game.hideOnscreenControllers();
        } else if (game.onscreenControlsShown() && !game.isHidingOverlays && game.conn != null && game.conn.canSendInput()) {
            game.showOnscreenController();
        }
        // Ending while the settings keep the controls away, as a pad in hand does, leaves them as they are: they went
        // when the moving began, and hiding them now would close their open editor and drop its draft.
    }

    void hideOnscreenControllers() {
        if (game.modernController != null) {
            // Moving the game buttons puts the controls aside only for a while: an open editor of theirs is the
            // player's work and keeps its draft.
            if (!movingGameButtons()) game.modernController.finishEditing(false);
            game.modernController.hide();
        }
        if (game.micButton != null) game.micButton.place();
        if (game.menuButton != null) game.menuButton.place();
        if (game.performanceOverlayScroll != null) game.performanceOverlayScroll.requestLayout();
    }

    // Creates the on-screen controller, or brings the existing one back.
    void showOnscreenController() {
        if (!game.onscreenControlsShown()) {
            game.hideOnscreenControllers();
            return;
        }
        ensureModernController();
        // The stream may have turned while a pad kept the controls off the screen, so the switch brings this
        // orientation's layout back with the controls, as onConfigurationChanged would have.
        game.modernController.onOrientationChanged();
        game.modernController.show();
        // The controls came back: the microphone and menu buttons follow the top row again, as after hiding.
        game.touchInput.updateMicButton();
        game.touchInput.updateMenuButton();
    }

    /** Builds the on-screen controller without showing it, so its layout can be edited while a gamepad hides it. */
    private void ensureModernController() {
        if (game.modernController != null) return;
        String profile = TouchLayout.profileKey(game.getIntent().getStringExtra(Game.EXTRA_PC_UUID),
                game.app.getAppUuid(), game.app.getAppId());
        game.modernController = new ModernController(game,
                (FrameLayout) game.streamView.getParent(), new HeldInputState.Sink() {
                    // The WASD stick: Windows virtual-key codes, sent like the PC keyboard's keys.
                    @Override public void key(int key, boolean down) { game.keyboardInput.sendOnScreenKey(key, down); }
                    @Override public void mouse(int button, boolean down) {
                        if (game.conn != null) game.touchInput.sendCustomMouse(button, down);
                    }
                    @Override public void gamepad(int buttons, short leftX, short leftY, short rightX, short rightY,
                                                  byte leftTrigger, byte rightTrigger) {
                        // The on-screen gamepad's report.
                        if (game.controllerHandler != null) game.controllerHandler.reportOscState(buttons,
                                leftX, leftY, rightX, rightY, leftTrigger, rightTrigger);
                    }
                }, game.prefConfig, profile);
        // The layout of a connected controller that has one, before anything listens for layout changes.
        game.modernController.setControllers(game.controllerModels);
        // The Steam Controller trackpads, on touchpad 0 of the on-screen gamepad.
        game.modernController.setTouchSink((type, pointer, x, y, pressure) -> {
            if (game.controllerHandler != null) game.controllerHandler.reportOscTouch(type, pointer, x, y, pressure);
        });
        // The pointer touchpad: the screen touchpad's host-pixel scaling, Trackpad speed, kept fraction and zoom.
        game.modernController.setPointerSink(new ModernController.PointerSink() {
            @Override public void move(int dx, int dy) {
                if (game.conn == null || game.streamView.getWidth() <= 0 || game.streamView.getHeight() <= 0) return;
                float zoom = game.trackpadPointer != null && game.trackpadPointer.isLocalCursor() ?
                        game.trackpadPointer.zoomScale() : 1;
                game.pointerMove.setViewport(game.prefConfig.width, game.prefConfig.height, game.streamView.getWidth(),
                        game.streamView.getHeight(), game.prefConfig.trackpadSpeed, zoom);
                if (!game.pointerMove.move(dx, dy, pointerMoveDelta)) return;
                // The same route as the screen touchpad: a local cursor, or the mouse's absolute position, needs
                // trackpadPointer's own bookkeeping, and a plain mouse move is its fallback.
                if (game.trackpadPointer != null) {
                    game.trackpadPointer.stopMomentum();
                    game.trackpadPointer.moveBy((short) pointerMoveDelta[0], (short) pointerMoveDelta[1]);
                } else {
                    game.conn.sendMouseMove((short) pointerMoveDelta[0], (short) pointerMoveDelta[1]);
                }
            }
            @Override public void button(int button, boolean down) {
                if (game.conn == null) return;
                if (down) game.conn.sendMouseButtonDown((byte) button);
                else game.conn.sendMouseButtonUp((byte) button);
            }
            @Override public void scroll(int vertical, int horizontal) {
                game.touchInput.sendCustomScroll(vertical, horizontal);
            }
        });
        game.modernController.setOnLayoutChanged(() -> {
            if (game.micButton != null) game.micButton.place();
            if (game.menuButton != null) game.menuButton.place();
            // An editor opened while a gamepad kept the controls off brings them back only if it should; a layout
            // just saved for the connected controller brings them back.
            if (!onscreenControlsShown()) game.modernController.hide();
            else if (!game.modernController.isShown() && !game.modernController.isEditing() && !game.isHidingOverlays
                    && !game.isFinishing() && game.conn != null && game.conn.canSendInput()) game.showOnscreenController();
        });
        game.modernController.refreshLayout();
        // Editing may build the controls while a gamepad keeps them off the screen; the editor itself still shows.
        if (!onscreenControlsShown()) game.modernController.hide();
    }

    /** The on-screen controls exist, whether or not a gamepad is keeping them off the screen. */
    void ensureOnscreenControllers() {
        if (game.prefConfig == null) return;
        ensureModernController();
    }
}
