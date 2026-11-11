package com.limelight.binding.input.touch;

import android.os.Handler;
import android.os.Looper;

import android.view.View;

import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.input.MouseButtonPacket;

/**
 * Tap-to-click, double-tap drag, two-finger scroll and flick momentum for a physical touchpad (a
 * keyboard cover's, for example) whose finger touches reach the stream as SOURCE_TOUCHPAD events.
 * Ported from Artemis (ClassicOldSong/moonlight-android).
 */
public class TrackpadContext implements TouchContext {
    private double pendingDeltaX = 0;
    private double pendingDeltaY = 0;
    private int lastTouchX = 0;
    private int lastTouchY = 0;
    private int originalTouchX = 0;
    private int originalTouchY = 0;
    private long originalTouchTime = 0;
    private boolean cancelled;
    private boolean confirmedMove;
    private boolean confirmedDrag;
    private boolean confirmedScroll;
    private double distanceMoved;
    private int pointerCount;
    private boolean clickedMiddle = false;
    private int maxPointerCountInGesture;
    private boolean isClickPending;
    private boolean isDblClickPending;
    private boolean isFlicking;
    private double velocityX = 0.0;
    private double velocityY = 0.0;
    // Scrolling is linear in the finger movement, so it has its own velocity and no acceleration.
    private double scrollVelocityX = 0.0;
    private double scrollVelocityY = 0.0;
    private double scrollRemainderX, scrollRemainderY;
    private float scrollSpeed = 0.5f;
    // Input units per millimetre of finger travel, so scrolling feels the same on any touchpad or screen.
    private float unitsPerMm = 10f;
    private long lastMoveTime;
    private boolean isScrollTransitioning = false;

    private final NvConnection conn;
    private final int actionIndex;
    private final Handler handler;

    private View view;
    private MoveScale moveScale;
    private float moveScaleX = 1, moveScaleY = 1;
    private double moveRemainderX, moveRemainderY;
    private int referenceWidth, referenceHeight;
    private boolean swapAxis = false;
    private float sensitivityX = 1;
    private float sensitivityY = 1;

    private static final int TAP_MOVEMENT_THRESHOLD = 30;
    private static final int TAP_TIME_THRESHOLD = 230;
    private static final int CLICK_RELEASE_DELAY = TAP_TIME_THRESHOLD;
    // Unit: millimetres/ms.
    private static final double SCROLL_FLICK_THRESHOLD = 0.1;
    // High-resolution scroll units (120 is one notch) per millimetre of finger travel at 100%;
    // the default setting is 50%.
    private static final double SCROLL_UNITS_PER_MM = 1.2;
    private static final double HORIZONTAL_SCROLL_RATIO = 2.0 / 3.0;
    private static final double ACCELERATION_THRESHOLD = 8.0;
    private static final double FLICK_FRICTION = 0.93;
    // Unit: pixels/ms.
    private static final double FLICK_THRESHOLD = 0.8;
    private static final int MOMENTUM_FRAME_INTERVAL_MS = 10;
    private static final int FLICK_VELOCITY_DECAY_TIMEOUT_MS = 50;
    private static final int SCROLL_TRANSITION_TIMEOUT_MS = 200;

    public TrackpadContext(NvConnection conn, int actionIndex) {
        this.conn = conn;
        this.actionIndex = actionIndex;
        this.handler = new Handler(Looper.getMainLooper());
    }

    /**
     * @param swapAxis swap X and Y of the touchpad before the sensitivity applies
     * @param sensitivityX percent, negative flips the axis
     * @param sensitivityY percent, negative flips the axis
     * @param speed percent from Mouse speed, applied on top of the sensitivity
     * @param view stream view used to send movement as a position, or null to send it as relative movement
     */
    public TrackpadContext(NvConnection conn, int actionIndex, boolean swapAxis, int sensitivityX, int sensitivityY,
                           int speed, View view) {
        this(conn, actionIndex);
        this.swapAxis = swapAxis;
        this.sensitivityX = (float) sensitivityX / 100 * speed / 100;
        this.sensitivityY = (float) sensitivityY / 100 * speed / 100;
        this.view = view;
    }

    /** Extra scale for pointer movement only, read at the start of each touch; scrolling is not scaled. */
    public interface MoveScale {
        float x();
        float y();
    }

    /**
     * For a touchscreen: movement is scaled from screen pixels to the stream's resolution (and by
     * the trackpad speed), and sent as a position over the stream size when {@code absolute} is set.
     */
    public TrackpadContext(NvConnection conn, int actionIndex, boolean absolute, int referenceWidth, int referenceHeight,
                           MoveScale moveScale) {
        this(conn, actionIndex);
        this.moveScale = moveScale;
        this.referenceWidth = absolute ? referenceWidth : 0;
        this.referenceHeight = absolute ? referenceHeight : 0;
    }

    /** Resolution of the input in units per millimetre; ignored when not positive. */
    public void setUnitsPerMm(float unitsPerMm) {
        if (unitsPerMm > 0) {
            this.unitsPerMm = unitsPerMm;
        }
    }

    private double scrollGainY() {
        return SCROLL_UNITS_PER_MM * scrollSpeed / unitsPerMm;
    }

    private double scrollGainX() {
        return scrollGainY() * HORIZONTAL_SCROLL_RATIO;
    }

    /** @param percent how far two-finger scrolling goes per finger movement, from Trackpad Scroll Speed */
    public void setScrollSpeed(int percent) {
        this.scrollSpeed = percent / 100f;
    }

    /**
     * Sends scrolling in high-resolution units, keeping the fractions for the next call. An axis
     * that is not sent drops its fraction.
     */
    private void sendScroll(double unitsX, double unitsY, boolean horizontal, boolean vertical) {
        scrollRemainderX = horizontal ? scrollRemainderX + unitsX : 0;
        scrollRemainderY = vertical ? scrollRemainderY + unitsY : 0;
        short x = (short) scrollRemainderX;
        short y = (short) scrollRemainderY;
        scrollRemainderX -= x;
        scrollRemainderY -= y;
        if (x != 0) {
            conn.sendMouseHighResHScroll(x);
        }
        if (y != 0) {
            conn.sendMouseHighResScroll(y);
        }
    }

    private void sendMove(short deltaX, short deltaY) {
        // Scale here rather than on the raw deltas so that scrolling keeps its own speed.
        moveRemainderX += deltaX * moveScaleX;
        moveRemainderY += deltaY * moveScaleY;
        short x = (short) moveRemainderX;
        short y = (short) moveRemainderY;
        moveRemainderX -= x;
        moveRemainderY -= y;
        if (x == 0 && y == 0) {
            return;
        }
        if (referenceWidth > 0) {
            conn.sendMouseMoveAsMousePosition(x, y, (short) referenceWidth, (short) referenceHeight);
        } else if (view != null) {
            conn.sendMouseMoveAsMousePosition(x, y, (short) view.getWidth(), (short) view.getHeight());
        } else {
            conn.sendMouseMove(x, y);
        }
    }

    private final Runnable scrollTransitionRunnable = new Runnable() {
        @Override
        public void run() {
            isScrollTransitioning = false;
        }
    };

    private final Runnable momentumRunnable = new Runnable() {
        @Override
        public void run() {
            if (!isFlicking) {
                return;
            }

            pendingDeltaX += velocityX * MOMENTUM_FRAME_INTERVAL_MS;
            pendingDeltaY += velocityY * MOMENTUM_FRAME_INTERVAL_MS;

            short intDeltaX = (short) pendingDeltaX;
            short intDeltaY = (short) pendingDeltaY;

            if (intDeltaX != 0 || intDeltaY != 0) {
                sendMove(intDeltaX, intDeltaY);
                pendingDeltaX -= intDeltaX;
                pendingDeltaY -= intDeltaY;
            }

            velocityX *= FLICK_FRICTION;
            velocityY *= FLICK_FRICTION;

            if (Math.sqrt(velocityX * velocityX + velocityY * velocityY) * MOMENTUM_FRAME_INTERVAL_MS < 0.5) {
                isFlicking = false;
                if (confirmedDrag) {
                    conn.sendMouseButtonUp(getMouseButtonIndex());
                    confirmedDrag = false;
                }
            }

            if (isFlicking) {
                handler.postDelayed(this, MOMENTUM_FRAME_INTERVAL_MS);
            }
        }
    };

    private final Runnable scrollMomentumRunnable = new Runnable() {
        @Override
        public void run() {
            if (!isFlicking) {
                return;
            }

            double frameX = scrollVelocityX * MOMENTUM_FRAME_INTERVAL_MS;
            double frameY = scrollVelocityY * MOMENTUM_FRAME_INTERVAL_MS;
            double unitsX = -frameX * scrollGainX();
            double unitsY = frameY * scrollGainY();

            if (Math.abs(frameX) > Math.abs(frameY)) {
                sendScroll(unitsX, unitsY, true, Math.abs(frameY) * 1.05 > Math.abs(frameX));
            } else {
                sendScroll(unitsX, unitsY, Math.abs(frameX) * 1.05 >= Math.abs(frameY), true);
            }

            scrollVelocityX *= FLICK_FRICTION;
            scrollVelocityY *= FLICK_FRICTION;

            if (Math.hypot(unitsX, unitsY) < 0.5) {
                isFlicking = false;
            }

            if (isFlicking) {
                handler.postDelayed(this, MOMENTUM_FRAME_INTERVAL_MS);
            }
        }
    };

    @Override
    public int getActionIndex() {
        return actionIndex;
    }

    private boolean isWithinTapBounds(int touchX, int touchY) {
        int xDelta = Math.abs(touchX - originalTouchX);
        int yDelta = Math.abs(touchY - originalTouchY);
        return xDelta <= TAP_MOVEMENT_THRESHOLD && yDelta <= TAP_MOVEMENT_THRESHOLD;
    }

    private boolean isTap(long eventTime) {
        if (confirmedDrag || confirmedMove || confirmedScroll) {
            return false;
        }

        if (actionIndex + 1 != maxPointerCountInGesture) {
            return false;
        }

        long timeDelta = eventTime - originalTouchTime;
        return isWithinTapBounds(lastTouchX, lastTouchY) && timeDelta <= TAP_TIME_THRESHOLD;
    }

    private byte getMouseButtonIndex() {
        if (pointerCount == 2) {
            return MouseButtonPacket.BUTTON_RIGHT;
        } else if (pointerCount == 3) {
            clickedMiddle = true;
            return MouseButtonPacket.BUTTON_MIDDLE;
        } else {
            return MouseButtonPacket.BUTTON_LEFT;
        }
    }

    @Override
    public boolean touchDownEvent(int eventX, int eventY, long eventTime, boolean isNewFinger) {
        if (moveScale != null) {
            // The view may have been resized since the last touch.
            moveScaleX = moveScale.x();
            moveScaleY = moveScale.y();
        }
        if (isFlicking) {
            isFlicking = false;
            handler.removeCallbacksAndMessages(null);
        }

        originalTouchX = lastTouchX = eventX;
        originalTouchY = lastTouchY = eventY;

        pendingDeltaX = pendingDeltaY = 0;

        if (isNewFinger) {
            // A completely new gesture has started.
            // Cancel any pending scroll->move transition.
            clickedMiddle = false;
            handler.removeCallbacks(scrollTransitionRunnable);
            isScrollTransitioning = false;
            maxPointerCountInGesture = pointerCount;
            originalTouchTime = eventTime;
            cancelled = confirmedMove = confirmedScroll = false;
            distanceMoved = 0;
            velocityX = 0;
            velocityY = 0;
            scrollVelocityX = 0;
            scrollVelocityY = 0;
            scrollRemainderX = 0;
            scrollRemainderY = 0;
            lastMoveTime = eventTime;
            if (isClickPending) {
                isClickPending = false;
                isDblClickPending = true;
                confirmedDrag = true;
            }
        } else {
            if (pointerCount == 2 && !confirmedMove) {
                conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_MIDDLE);
                conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_MIDDLE);
                isClickPending = false;
                isDblClickPending = false;
                confirmedDrag = false;
                clickedMiddle = true;
            // Second finger released, should trigger right click immediately
            } else if (pointerCount == 1 && !confirmedMove && !clickedMiddle) {
                conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
                conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
                isClickPending = false;
                isDblClickPending = false;
                confirmedDrag = false;
            }
        }

        originalTouchX = lastTouchX = eventX;
        originalTouchY = lastTouchY = eventY;

        pendingDeltaX = pendingDeltaY = 0;

        return true;
    }

    @Override
    public void touchUpEvent(int eventX, int eventY, long eventTime) {
        if (cancelled) {
            return;
        }

        // Decay velocity based on time since last move event to avoid
        // flicks when the user pauses before lifting their finger.
        long timeSinceLastMove = eventTime - lastMoveTime;
        if (timeSinceLastMove > 0) {
            double decay = Math.max(0.0, 1.0 - (double)timeSinceLastMove / FLICK_VELOCITY_DECAY_TIMEOUT_MS);
            velocityX *= decay;
            velocityY *= decay;
            scrollVelocityX *= decay;
            scrollVelocityY *= decay;
        }

        byte buttonIndex = getMouseButtonIndex();

        if (isDblClickPending) {
            handler.removeCallbacksAndMessages(null);
            conn.sendMouseButtonUp(buttonIndex);
            conn.sendMouseButtonDown(buttonIndex);
            conn.sendMouseButtonUp(buttonIndex);
            isClickPending = false;
            confirmedDrag = false;
        }
        else if (confirmedDrag) {
            handler.removeCallbacksAndMessages(null);

            double speed = Math.sqrt(velocityX * velocityX + velocityY * velocityY);
            if (speed > FLICK_THRESHOLD) {
                isFlicking = true;
                handler.post(momentumRunnable);
            } else {
                conn.sendMouseButtonUp(buttonIndex);
                confirmedDrag = false;
            }
        }
        else if (isTap(eventTime)) {
            conn.sendMouseButtonDown(buttonIndex);
            isClickPending = true;

            handler.removeCallbacksAndMessages(null);
            handler.postDelayed(() -> {
                if (isClickPending) {
                    conn.sendMouseButtonUp(buttonIndex);
                    isClickPending = false;
                }
                isDblClickPending = false;
            }, CLICK_RELEASE_DELAY);
        }
        else if (confirmedMove) {
            // This was a move/scroll that wasn't a drag or tap. Let's see if we should flick.
            double speed = Math.sqrt(velocityX * velocityX + velocityY * velocityY);
            if (confirmedScroll) {
                if (Math.hypot(scrollVelocityX, scrollVelocityY) / unitsPerMm > SCROLL_FLICK_THRESHOLD) {
                    isFlicking = true;
                    handler.post(scrollMomentumRunnable);
                }
            } else if (speed > FLICK_THRESHOLD) {
                // A 1-finger move can flick. A >1 finger move that wasn't a scroll shouldn't cause a mouse move flick.
                if (maxPointerCountInGesture == 1) {
                    isFlicking = true;
                    handler.post(momentumRunnable);
                }
            }
        }
    }

    @Override
    public boolean touchMoveEvent(int eventX, int eventY, long eventTime) {
        if (cancelled) {
            return true;
        }

        if (eventX != lastTouchX || eventY != lastTouchY) {
            long deltaTime = eventTime - lastMoveTime;

            checkForConfirmedMove(eventX, eventY);

            if (isDblClickPending) {
                isDblClickPending = false;
                confirmedDrag = true;
            }

            int rawDeltaX = eventX - lastTouchX;
            int rawDeltaY = eventY - lastTouchY;
            int absDeltaX, absDeltaY;

            double magnitude = Math.sqrt(rawDeltaX * rawDeltaX + rawDeltaY * rawDeltaY);
            double precisionMultiplier = Math.cbrt(magnitude / ACCELERATION_THRESHOLD);

            float deltaX, deltaY;
            if (swapAxis) {
                deltaY = rawDeltaX;
                deltaX = rawDeltaY;

                absDeltaX = Math.abs(rawDeltaY);
                absDeltaY = Math.abs(rawDeltaX);
            } else {
                deltaX = rawDeltaX;
                deltaY = rawDeltaY;

                absDeltaX = Math.abs(rawDeltaX);
                absDeltaY = Math.abs(rawDeltaY);
            }

            // Scrolling follows the finger linearly: no acceleration, but the axis swap, direction
            // and sensitivity of the touchpad still apply.
            float scrollDeltaX = deltaX * sensitivityX;
            float scrollDeltaY = deltaY * sensitivityY;

            deltaX *= precisionMultiplier;
            deltaY *= precisionMultiplier;

            deltaX *= sensitivityX;
            deltaY *= sensitivityY;

            // Update velocity for flicking
            if (deltaTime > 0 && (confirmedMove || confirmedDrag)) {
                double currentVelocityX = deltaX / deltaTime;
                double currentVelocityY = deltaY / deltaTime;

                if (velocityX == 0 && velocityY == 0) {
                    // First measurement
                    velocityX = currentVelocityX;
                    velocityY = currentVelocityY;
                } else {
                    // Simple EMA for smoothing
                    velocityX = velocityX * 0.8 + currentVelocityX * 0.2;
                    velocityY = velocityY * 0.8 + currentVelocityY * 0.2;
                }

                double currentScrollX = scrollDeltaX / deltaTime;
                double currentScrollY = scrollDeltaY / deltaTime;
                if (scrollVelocityX == 0 && scrollVelocityY == 0) {
                    scrollVelocityX = currentScrollX;
                    scrollVelocityY = currentScrollY;
                } else {
                    scrollVelocityX = scrollVelocityX * 0.8 + currentScrollX * 0.2;
                    scrollVelocityY = scrollVelocityY * 0.8 + currentScrollY * 0.2;
                }
            }

            lastMoveTime = eventTime;

            pendingDeltaX += deltaX;
            pendingDeltaY += deltaY;

            lastTouchX = eventX;
            lastTouchY = eventY;

            short sendDeltaX = (short)pendingDeltaX;
            short sendDeltaY = (short)pendingDeltaY;

            if (pointerCount == 1) {
                // Don't move the mouse immediately after a scroll
                if (!isScrollTransitioning) {
                    if (sendDeltaX != 0 || sendDeltaY != 0) {
                        sendMove(sendDeltaX, sendDeltaY);
                    }
                }
            } else {
                if (actionIndex == 1) {
                    if (confirmedDrag) {
                        if (sendDeltaX != 0 || sendDeltaY != 0) {
                            sendMove(sendDeltaX, sendDeltaY);
                        }
                    } else if (pointerCount == 2) {
                        checkForConfirmedScroll();
                        if (confirmedScroll) {
                            double unitsX = -scrollDeltaX * scrollGainX();
                            double unitsY = scrollDeltaY * scrollGainY();
                            if (absDeltaX > absDeltaY) {
                                sendScroll(unitsX, unitsY, true, absDeltaY * 1.05 > absDeltaX);
                            } else {
                                sendScroll(unitsX, unitsY, absDeltaX * 1.05 >= absDeltaY, true);
                            }
                        }
                    }
                }
            }

            pendingDeltaX -= sendDeltaX;
            pendingDeltaY -= sendDeltaY;
        }

        return true;
    }

    @Override
    public void cancelTouch() {
        cancelled = true;

        if (isFlicking) {
            isFlicking = false;
            handler.removeCallbacksAndMessages(null);
        }

        if (confirmedDrag) {
            conn.sendMouseButtonUp(getMouseButtonIndex());
        }
    }

    @Override
    public boolean isCancelled() {
        return cancelled;
    }

    @Override
    public void setPointerCount(int pointerCount) {
        if (this.pointerCount == 2 && pointerCount == 1) {
            // We just finished a 2-finger scroll.
            // Block mouse movement for a short period to avoid stray movement
            // from the remaining finger before it's lifted.
            isScrollTransitioning = true;
            handler.postDelayed(scrollTransitionRunnable, SCROLL_TRANSITION_TIMEOUT_MS);
        } else if (this.pointerCount == 1 && pointerCount == 2) {
            // User put a second finger down. If we were in a scroll transition, cancel it.
            handler.removeCallbacks(scrollTransitionRunnable);
            isScrollTransitioning = false;
        }

        if (pointerCount < this.pointerCount && confirmedDrag && !isFlicking) {
            conn.sendMouseButtonUp(getMouseButtonIndex());
            confirmedDrag = false;
            confirmedMove = false;
            confirmedScroll = false;
            isClickPending = false;
            isDblClickPending = false;
        }

        this.pointerCount = pointerCount;

        if (pointerCount > maxPointerCountInGesture) {
            maxPointerCountInGesture = pointerCount;
        }
    }

    private void checkForConfirmedMove(int eventX, int eventY) {
        // If we've already confirmed something, get out now
        if (confirmedMove || confirmedDrag) {
            return;
        }

        // If it leaves the tap bounds before the drag time expires, it's a move.
        if (!isWithinTapBounds(eventX, eventY)) {
            confirmedMove = true;
            return;
        }

        // Check if we've exceeded the maximum distance moved
        distanceMoved += Math.sqrt(Math.pow(eventX - lastTouchX, 2) + Math.pow(eventY - lastTouchY, 2));
        if (distanceMoved >= TAP_MOVEMENT_THRESHOLD) {
            confirmedMove = true;
        }
    }

    private void checkForConfirmedScroll() {
        confirmedScroll = (actionIndex == 1 && pointerCount == 2 && confirmedMove);
    }
}
