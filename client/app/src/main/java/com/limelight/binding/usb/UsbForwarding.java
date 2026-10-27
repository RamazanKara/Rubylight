package com.limelight.binding.usb;

import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.hardware.usb.UsbConstants;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

import androidx.core.content.ContextCompat;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Hands USB devices plugged into this device to the PC for one stream (the Rust engine's
 * forwarder does the rest). Main thread only; attaching runs on its own thread.
 */
public final class UsbForwarding {
    // Not the input drivers' "com.limelight.USB_PERMISSION": their receiver would open the device too.
    static final String ACTION_PERMISSION = "com.limelight.USB_FORWARDING_PERMISSION";
    private static final int PENDING = -1;

    /** The Rust engine's forwarder; MoonBridge outside tests. */
    interface Bridge {
        int attach(int fd, String path, String name);
        boolean detach(int requestId);
        String list();
        void setListener(Runnable listener);
    }

    /** An open device; a UsbDeviceConnection outside tests. */
    interface Connection {
        int fd();
        void close();
    }

    interface Opener {
        /** Null when Android does not open it. */
        Connection open(UsbDevice device);
    }

    /** Where the user reads what happened. */
    public interface Messages {
        void show(CharSequence text);
    }

    /** One entry of usbList(), as the Rust engine's usb.rs list_json writes it. */
    static final class Attachment {
        final int requestId;
        final String name;
        /** asking, opening, attached or ended. */
        final String phase;
        /** Once ended: attached, detached, refused or failed, the host's reason, and what failed here. */
        final String state, reason, error;

        Attachment(int requestId, String name, String phase, String state, String reason, String error) {
            this.requestId = requestId;
            this.name = name;
            this.phase = phase;
            this.state = state;
            this.reason = reason;
            this.error = error;
        }

        boolean ended() {
            return "ended".equals(phase);
        }

        /** Changes whenever there is something new to say about it. */
        String key() {
            return phase + "/" + state + "/" + reason + "/" + error;
        }

        static List<Attachment> parseList(String json) throws JSONException {
            JSONArray array = new JSONArray(json);
            List<Attachment> list = new ArrayList<>(array.length());
            for (int i = 0; i < array.length(); i++) {
                JSONObject entry = array.getJSONObject(i);
                list.add(new Attachment(entry.getInt("requestId"), entry.getString("name"), entry.getString("phase"),
                        entry.optString("state", null), entry.optString("reason", null), entry.optString("error", null)));
            }
            return list;
        }
    }

    private static final class Forwarded {
        final String name;
        final Connection connection;
        int requestId = PENDING;
        // Until usbAttach returns: the engine may not have copied the descriptor yet.
        boolean attaching = true;
        String announced = "";

        Forwarded(String name, Connection connection) {
            this.name = name;
            this.connection = connection;
        }
    }

    private final Context context;
    private final UsbManager usbManager;
    private final PreferenceConfiguration prefConfig;
    private final Bridge bridge;
    private final Opener opener;
    private final ExecutorService worker;
    private final Messages messages;
    private final Handler main = new Handler(Looper.getMainLooper());
    // By usbfs path, while it is forwarded or being attached.
    private final Map<String, Forwarded> forwarded = new LinkedHashMap<>();
    private final Set<String> asking = new HashSet<>();
    private boolean started;
    private boolean stopped;

    private final BroadcastReceiver receiver = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            UsbDevice device = intent.getParcelableExtra(UsbManager.EXTRA_DEVICE);
            if (device == null || stopped) return;
            if (UsbManager.ACTION_USB_DEVICE_DETACHED.equals(intent.getAction())) {
                unplugged(device);
            } else if (ACTION_PERMISSION.equals(intent.getAction()) && asking.remove(device.getDeviceName())) {
                if (intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)) {
                    open(device);
                } else {
                    say(R.string.usb_forwarding_permission_denied, name(device));
                }
            }
        }
    };

    public UsbForwarding(Context context, PreferenceConfiguration prefConfig, NvConnection conn, Messages messages) {
        this(context, (UsbManager) context.getSystemService(Context.USB_SERVICE), prefConfig, new Bridge() {
            @Override public int attach(int fd, String path, String name) {
                return MoonBridge.usbAttach(fd, path, name, conn.hostEndpoint());
            }
            @Override public boolean detach(int requestId) {
                return MoonBridge.usbDetach(requestId);
            }
            @Override public String list() {
                return MoonBridge.usbList();
            }
            @Override public void setListener(Runnable listener) {
                MoonBridge.setUsbListener(listener);
            }
        }, null, Executors.newSingleThreadExecutor(), messages);
    }

    UsbForwarding(Context context, UsbManager usbManager, PreferenceConfiguration prefConfig, Bridge bridge,
                  Opener opener, ExecutorService worker, Messages messages) {
        this.context = context;
        this.usbManager = usbManager;
        this.prefConfig = prefConfig;
        this.bridge = bridge;
        this.opener = opener != null ? opener : this::openConnection;
        this.worker = worker;
        this.messages = messages;
    }

    /** What the user may forward: no hubs, nothing the input drivers take. */
    public List<UsbDevice> candidates() {
        List<UsbDevice> devices = new ArrayList<>();
        if (usbManager == null) return devices;
        for (UsbDevice device : usbManager.getDeviceList().values()) {
            if (device.getDeviceClass() == UsbConstants.USB_CLASS_HUB) continue;
            if (UsbDriverService.shouldClaimDevice(device, prefConfig)) continue;
            devices.add(device);
        }
        Collections.sort(devices, (a, b) -> a.getDeviceName().compareTo(b.getDeviceName()));
        return devices;
    }

    /** True while the device is forwarded or being attached: choosing it again detaches it. */
    public boolean isForwarded(UsbDevice device) {
        return forwarded.containsKey(device.getDeviceName());
    }

    /** The picker's row for it. */
    public CharSequence label(UsbDevice device) {
        return isForwarded(device) ? context.getString(R.string.usb_forwarding_detach, name(device)) : name(device);
    }

    /** The user picked it: attaches it, asking for permission first, or detaches it when it is forwarded. */
    public void choose(UsbDevice device) {
        if (stopped) return;
        start();
        Forwarded entry = forwarded.get(device.getDeviceName());
        if (entry != null) {
            if (entry.attaching) {
                say(R.string.usb_forwarding_asking, entry.name);
            } else if (!bridge.detach(entry.requestId)) {
                // Not attached any more: the list says why, or it never got that far.
                end(device.getDeviceName(), entry);
                say(R.string.usb_forwarding_detached, entry.name);
            }
            // Otherwise the forwarder's list says when it is detached.
            return;
        }
        if (usbManager.hasPermission(device)) {
            open(device);
            return;
        }
        if (!asking.add(device.getDeviceName())) return;
        try {
            int flags = 0;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                // Mutable so that the framework can add EXTRA_DEVICE and EXTRA_PERMISSION_GRANTED.
                flags |= PendingIntent.FLAG_MUTABLE;
            }
            // Explicit for the unexported receiver, as Android 14 requires.
            Intent intent = new Intent(ACTION_PERMISSION).setPackage(context.getPackageName());
            usbManager.requestPermission(device, PendingIntent.getBroadcast(context, device.getDeviceId(), intent, flags));
        } catch (SecurityException e) {
            // Samsung Knox policies throw this instead of denying.
            asking.remove(device.getDeviceName());
            LimeLog.severe("USB: no permission request for " + device.getDeviceName() + ": " + e);
            messages.show(context.getText(R.string.error_usb_prohibited));
        }
    }

    /** The stream ends: detaches everything and closes this side's connections. */
    public void stop() {
        if (stopped) return;
        stopped = true;
        if (started) {
            bridge.setListener(null);
            context.unregisterReceiver(receiver);
        }
        for (Map.Entry<String, Forwarded> e : new ArrayList<>(forwarded.entrySet())) {
            if (e.getValue().requestId != PENDING) bridge.detach(e.getValue().requestId);
            end(e.getKey(), e.getValue());
        }
        asking.clear();
        // An attach still on its way finds its entry gone, detaches what it attached and closes.
        worker.shutdown();
    }

    private void start() {
        if (started) return;
        started = true;
        IntentFilter filter = new IntentFilter();
        filter.addAction(UsbManager.ACTION_USB_DEVICE_DETACHED);
        filter.addAction(ACTION_PERMISSION);
        ContextCompat.registerReceiver(context, receiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED);
        bridge.setListener(() -> main.post(this::refresh));
    }

    private void open(UsbDevice device) {
        String path = device.getDeviceName();
        String name = name(device);
        if (forwarded.containsKey(path)) return;
        Connection connection;
        try {
            connection = opener.open(device);
        } catch (SecurityException e) {
            LimeLog.severe("USB: " + path + " did not open: " + e);
            connection = null;
        }
        if (connection == null) {
            say(R.string.usb_forwarding_failed, name, context.getString(R.string.usb_forwarding_code_device));
            return;
        }
        Forwarded entry = new Forwarded(name, connection);
        forwarded.put(path, entry);
        say(R.string.usb_forwarding_asking, name);
        int fd = connection.fd();
        worker.execute(() -> {
            // Connects to the host's native API the first time, so not on the main thread.
            int result = bridge.attach(fd, path, name);
            main.post(() -> attached(path, entry, result));
        });
    }

    private void attached(String path, Forwarded entry, int result) {
        entry.attaching = false;
        if (forwarded.get(path) != entry) {
            // Unplugged or stopped while it was being attached.
            if (result >= 0) bridge.detach(result);
            entry.connection.close();
            return;
        }
        if (result < 0) {
            end(path, entry);
            say(R.string.usb_forwarding_failed, entry.name, context.getString(codeReason(result)));
            return;
        }
        entry.requestId = result;
        // The list may have changed before the request id was known here.
        refresh();
    }

    private void unplugged(UsbDevice device) {
        String path = device.getDeviceName();
        asking.remove(path);
        Forwarded entry = forwarded.get(path);
        if (entry == null) return;
        if (entry.requestId != PENDING) bridge.detach(entry.requestId);
        end(path, entry);
        say(R.string.usb_forwarding_unplugged, entry.name);
    }

    // Says what changed in the forwarder's list.
    private void refresh() {
        if (stopped || forwarded.isEmpty()) return;
        List<Attachment> list;
        try {
            list = Attachment.parseList(bridge.list());
        } catch (JSONException e) {
            LimeLog.severe("USB: unreadable attachment list: " + e);
            messages.show(context.getText(R.string.usb_forwarding_list_failed));
            return;
        }
        for (Attachment attachment : list) {
            for (Map.Entry<String, Forwarded> e : new ArrayList<>(forwarded.entrySet())) {
                Forwarded entry = e.getValue();
                if (entry.requestId != attachment.requestId || entry.announced.equals(attachment.key())) continue;
                entry.announced = attachment.key();
                if ("attached".equals(attachment.phase)) {
                    say(R.string.usb_forwarding_attached, entry.name);
                } else if (attachment.ended()) {
                    end(e.getKey(), entry);
                    announceEnd(entry.name, attachment);
                }
            }
        }
    }

    private void announceEnd(String name, Attachment attachment) {
        String why = attachment.error != null ? attachment.error : reasonText(attachment.reason);
        if ("detached".equals(attachment.state) && why == null) {
            say(R.string.usb_forwarding_detached, name);
        } else if ("refused".equals(attachment.state)) {
            say(R.string.usb_forwarding_refused, name, why != null ? why : context.getString(R.string.usb_forwarding_reason_unknown));
        } else {
            say(R.string.usb_forwarding_failed, name, why != null ? why : context.getString(R.string.usb_forwarding_reason_unknown));
        }
    }

    private void end(String path, Forwarded entry) {
        forwarded.remove(path);
        // The forwarder has its own copy of the descriptor; while it is attaching, attached() closes it.
        if (!entry.attaching) entry.connection.close();
    }

    private String reasonText(String reason) {
        int text = reasonString(reason);
        return text == 0 ? null : context.getString(text);
    }

    /** The host's reason for ending it (usb.rs reason_name); 0 for none. */
    static int reasonString(String reason) {
        if (reason == null) return 0;
        switch (reason) {
            case "none": return 0;
            case "permission": return R.string.usb_forwarding_reason_permission;
            case "disabled": return R.string.usb_forwarding_reason_disabled;
            case "limit": return R.string.usb_forwarding_reason_limit;
            case "driver": return R.string.usb_forwarding_reason_driver;
            case "tunnel_timeout": return R.string.usb_forwarding_reason_timeout;
            case "device_error": return R.string.usb_forwarding_reason_device;
            case "session_end": return R.string.usb_forwarding_reason_session;
            default:
                LimeLog.severe("USB: unknown reason " + reason);
                return R.string.usb_forwarding_reason_unknown;
        }
    }

    /** Why usbAttach refused (MoonBridge.USB_*). */
    static int codeReason(int code) {
        switch (code) {
            case MoonBridge.USB_NO_STREAM: return R.string.usb_forwarding_code_no_stream;
            case MoonBridge.USB_BUSY: return R.string.usb_forwarding_code_busy;
            case MoonBridge.USB_UNSUPPORTED: return R.string.usb_forwarding_code_unsupported;
            case MoonBridge.USB_DEVICE_FAILED: return R.string.usb_forwarding_code_device;
            case MoonBridge.USB_INVALID: return R.string.usb_forwarding_code_invalid;
            case MoonBridge.USB_HOST_FAILED: return R.string.usb_forwarding_code_host;
            default:
                LimeLog.severe("USB: unknown attach result " + code);
                return R.string.usb_forwarding_reason_unknown;
        }
    }

    /** What the user calls it: the product name, else its ids. */
    public static String name(UsbDevice device) {
        String product = device.getProductName();
        if (product != null && !product.trim().isEmpty()) return product.trim();
        return String.format(Locale.ROOT, "USB %04x:%04x", device.getVendorId(), device.getProductId());
    }

    private void say(int text, Object... args) {
        messages.show(context.getString(text, args));
    }

    private Connection openConnection(UsbDevice device) {
        UsbDeviceConnection connection = usbManager.openDevice(device);
        if (connection == null) return null;
        return new Connection() {
            @Override public int fd() {
                return connection.getFileDescriptor();
            }
            @Override public void close() {
                connection.close();
            }
        };
    }
}
