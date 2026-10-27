package com.limelight.binding.input.evdev;

import android.app.Activity;
import android.net.LocalServerSocket;
import android.net.LocalSocket;
import android.os.Build;
import android.os.Looper;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructPollfd;
import android.widget.Toast;

import com.limelight.LimeLog;
import com.limelight.binding.input.capture.InputCaptureProvider;

import java.io.DataOutputStream;
import java.io.Closeable;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.UUID;

public class EvdevCaptureProvider extends InputCaptureProvider {

    private final EvdevListener listener;
    private final String libraryPath;

    private volatile boolean shutdown = false;
    private volatile InputStream evdevIn;
    private volatile OutputStream evdevOut;
    private volatile Process su;
    private volatile LocalServerSocket servSock;
    private volatile LocalSocket evdevSock;
    private Activity activity;
    private boolean started = false;

    private static final byte UNGRAB_REQUEST = 1;
    private static final byte REGRAB_REQUEST = 2;

    private final Thread handlerThread = new Thread() {
        @Override
        public void run() {
            int deltaX = 0;
            int deltaY = 0;
            byte deltaVScroll = 0;
            byte deltaHScroll = 0;

            // A separate Unix socket per launch lets both ends verify kernel peer credentials.
            final String socketName = "nightjar.evdev." + UUID.randomUUID();
            try {
                servSock = new LocalServerSocket(socketName);
            } catch (IOException e) {
                e.printStackTrace();
                return;
            }
            if (shutdown) {
                closeResources();
                return;
            }

            final String evdevReaderCmd = "exec " + shellQuote(libraryPath + File.separatorChar + "libevdev_reader.so") +
                    " " + shellQuote(socketName) + " " + android.os.Process.myUid();

            // On Nougat and later, we'll need to pass the command directly to SU.
            // Writing to SU's input stream after it has started doesn't seem to work anymore.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                // Launch evdev_reader directly via SU
                try {
                    su = new ProcessBuilder("su", "-c", evdevReaderCmd).start();
                    if (shutdown) {
                        su.destroy();
                        return;
                    }
                } catch (IOException e) {
                    reportDeviceNotRooted();
                    e.printStackTrace();
                    closeResources();
                    return;
                }
            }
            else {
                // Launch a SU shell on Marshmallow and earlier
                ProcessBuilder builder = new ProcessBuilder("su");
                builder.redirectErrorStream(true);

                try {
                    su = builder.start();
                    if (shutdown) {
                        su.destroy();
                        return;
                    }
                } catch (IOException e) {
                    reportDeviceNotRooted();
                    e.printStackTrace();
                    closeResources();
                    return;
                }

                // Start evdevreader
                DataOutputStream suOut = new DataOutputStream(su.getOutputStream());
                try {
                    suOut.write((evdevReaderCmd+"\n").getBytes(StandardCharsets.UTF_8));
                } catch (IOException e) {
                    reportDeviceNotRooted();
                    e.printStackTrace();
                    closeResources();
                    return;
                }
            }

            // Wait for evdevreader's connection
            if (shutdown) {
                closeResources();
                return;
            }
            LimeLog.info("Waiting for authenticated EvdevReader connection");
            try {
                evdevSock = acceptAuthenticatedHelper(servSock);
                servSock.close();
                if (shutdown || evdevSock == null) {
                    closeResources();
                    return;
                }
                evdevIn = evdevSock.getInputStream();
                evdevOut = evdevSock.getOutputStream();
            } catch (IOException | ErrnoException e) {
                if (!shutdown) e.printStackTrace();
                closeResources();
                return;
            }
            LimeLog.info("Authenticated EvdevReader connected");

            while (!isInterrupted() && !shutdown) {
                EvdevEvent event;
                try {
                    event = EvdevReader.read(evdevIn);
                } catch (IOException e) {
                    event = null;
                }
                if (event == null) {
                    break;
                }

                // Note: The EvdevReader process already filters input events when grabbing
                // is not enabled, so we don't need to that here.

                switch (event.type) {
                    case EvdevEvent.EV_SYN:
                        if (deltaX != 0 || deltaY != 0) {
                            final int x = deltaX, y = deltaY;
                            dispatch(() -> listener.mouseMove(x, y));
                            deltaX = deltaY = 0;
                        }
                        if (deltaVScroll != 0) {
                            final byte scroll = deltaVScroll;
                            dispatch(() -> listener.mouseVScroll(scroll));
                            deltaVScroll = 0;
                        }
                        if (deltaHScroll != 0) {
                            final byte scroll = deltaHScroll;
                            dispatch(() -> listener.mouseHScroll(scroll));
                            deltaHScroll = 0;
                        }
                        break;

                    case EvdevEvent.EV_REL:
                        switch (event.code) {
                            case EvdevEvent.REL_X:
                                deltaX = event.value;
                                break;
                            case EvdevEvent.REL_Y:
                                deltaY = event.value;
                                break;
                            case EvdevEvent.REL_HWHEEL:
                                deltaHScroll = (byte) event.value;
                                break;
                            case EvdevEvent.REL_WHEEL:
                                deltaVScroll = (byte) event.value;
                                break;
                        }
                        break;

                    case EvdevEvent.EV_KEY:
                        final boolean down = event.value != 0;
                        switch (event.code) {
                            case EvdevEvent.BTN_LEFT:
                                dispatch(() -> listener.mouseButtonEvent(EvdevListener.BUTTON_LEFT, down));
                                break;
                            case EvdevEvent.BTN_MIDDLE:
                                dispatch(() -> listener.mouseButtonEvent(EvdevListener.BUTTON_MIDDLE, down));
                                break;
                            case EvdevEvent.BTN_RIGHT:
                                dispatch(() -> listener.mouseButtonEvent(EvdevListener.BUTTON_RIGHT, down));
                                break;

                            case EvdevEvent.BTN_SIDE:
                                dispatch(() -> listener.mouseButtonEvent(EvdevListener.BUTTON_X1, down));
                                break;

                            case EvdevEvent.BTN_EXTRA:
                                dispatch(() -> listener.mouseButtonEvent(EvdevListener.BUTTON_X2, down));
                                break;

                            case EvdevEvent.BTN_FORWARD:
                            case EvdevEvent.BTN_BACK:
                            case EvdevEvent.BTN_TASK:
                                // Other unhandled mouse buttons
                                break;

                            default:
                                // We got some unrecognized button. This means
                                // someone is trying to use the other device in this
                                // "combination" input device. We'll try to handle
                                // it via keyboard, but we're not going to disconnect
                                // if we can't
                                short keyCode = EvdevTranslator.translateEvdevKeyCode(event.code);
                                if (keyCode != 0) {
                                    dispatch(() -> listener.keyboardEvent(down, keyCode));
                                }
                                break;
                        }
                        break;

                    case EvdevEvent.EV_MSC:
                        break;
                }
            }
            closeResources();
        }
    };

    public EvdevCaptureProvider(Activity activity, EvdevListener listener) {
        this.listener = listener;
        this.activity = activity;
        this.libraryPath = activity.getApplicationInfo().nativeLibraryDir;
    }

    private static String shellQuote(String value) {
        return "'" + value.replace("'", "'\\''") + "'";
    }

    private LocalSocket acceptAuthenticatedHelper(LocalServerSocket server) throws IOException, ErrnoException {
        StructPollfd pollfd = new StructPollfd();
        pollfd.fd = server.getFileDescriptor();
        pollfd.events = (short) OsConstants.POLLIN;
        StructPollfd[] pollfds = {pollfd};
        while (!shutdown) {
            final int ready;
            try {
                ready = Os.poll(pollfds, 1000);
            } catch (ErrnoException e) {
                if (e.errno == OsConstants.EINTR) continue;
                throw e;
            }
            if (shutdown) return null;
            if (ready == 0) {
                try {
                    su.exitValue();
                    reportDeviceNotRooted();
                    return null;
                } catch (IllegalThreadStateException stillRunning) {
                    // A pending superuser prompt can outlive the poll timeout.
                    continue;
                }
            }
            if ((pollfd.revents & OsConstants.POLLIN) == 0) {
                throw new IOException("EvdevReader listening socket closed");
            }
            LocalSocket candidate = server.accept();
            boolean authenticated = false;
            try {
                // Local apps cannot forge SO_PEERCRED. Only the root helper may send input.
                authenticated = candidate.getPeerCredentials().getUid() == 0 && !shutdown;
                if (authenticated) return candidate;
                LimeLog.warning("Rejected non-root EvdevReader peer");
            } finally {
                if (!authenticated) candidate.close();
            }
        }
        return null;
    }

    private void dispatch(Runnable event) {
        activity.runOnUiThread(() -> {
            if (!shutdown && isCapturing && !isCursorVisible) {
                event.run();
            }
        });
    }

    private void closeResources() {
        for (Closeable resource : new Closeable[] {servSock, evdevSock, evdevIn, evdevOut}) {
            if (resource != null) {
                try {
                    resource.close();
                } catch (IOException e) {
                    e.printStackTrace();
                }
            }
        }
        Process process = su;
        if (process != null) {
            process.destroy();
        }
    }

    private void reportDeviceNotRooted() {
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (!shutdown) {
                    Toast.makeText(activity, "This device is not rooted - Mouse capture is unavailable", Toast.LENGTH_LONG).show();
                }
            }
        });
    }

    private void runInNetworkSafeContextSynchronously(Runnable runnable) {
        // This function is used to avoid Android's strict NetworkOnMainThreadException.
        // For our usage, it is highly unlikely to cause problems since we only do
        // write operations and only to localhost sockets.
        if (Looper.getMainLooper().getThread() == Thread.currentThread()) {
            Thread t = new Thread(runnable);
            t.start();
            try {
                t.join();
            } catch (InterruptedException e) {
                // The main thread should never be interrupted
                e.printStackTrace();
            }
        }
        else {
            // Run the runnable directly
            runnable.run();
        }
    }

    @Override
    public void showCursor() {
        super.showCursor();
        // This may be called on the main thread
        runInNetworkSafeContextSynchronously(new Runnable() {
            @Override
            public void run() {
                if (started && !shutdown && evdevOut != null) {
                    try {
                        evdevOut.write(UNGRAB_REQUEST);
                    } catch (IOException e) {
                        e.printStackTrace();
                    }
                }
            }
        });
    }

    @Override
    public void hideCursor() {
        super.hideCursor();
        // This may be called on the main thread
        runInNetworkSafeContextSynchronously(new Runnable() {
            @Override
            public void run() {
                // Send a request to regrab if we're already capturing
                if (started && !shutdown && evdevOut != null) {
                    try {
                        evdevOut.write(REGRAB_REQUEST);
                    } catch (IOException e) {
                        e.printStackTrace();
                    }
                }
            }
        });
    }

    @Override
    public void enableCapture() {
        if (!started) {
            // Start the handler thread if it's our first time
            // capturing
            handlerThread.start();
            started = true;
        }

        // Call the superclass only after we've started the handler thread.
        // It will invoke hideCursor() when we call it.
        super.enableCapture();
    }

    @Override
    public void destroy() {
        // We need to stop the process in this context otherwise
        // we could get stuck waiting on output from the process
        // in order to terminate it.
        //
        // This may be called on the main thread.

        if (!started) {
            return;
        }

        shutdown = true;
        handlerThread.interrupt();

        runInNetworkSafeContextSynchronously(this::closeResources);

        try {
            handlerThread.join();
        } catch (InterruptedException ignored) {}
    }
}
