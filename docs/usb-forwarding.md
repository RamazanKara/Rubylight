# USB forwarding (Experimental)

A USB device plugged into the client, such as a flash drive, a USB gamepad or a dongle, can be attached to the host PC during a stream, as if it were plugged into the PC. The client serves the device over USB/IP inside a TLS tunnel on the stream's HTTPS port; on the host, [usbip-win2](https://github.com/vadimgrn/usbip-win2) attaches it. No USB/IP port is open to the network: the host relays the tunnel to the driver on 127.0.0.1. The protocol is in [native-api.md](native-api.md#usb-forwarding) and [control-messages.md](control-messages.md#0x553a-usb-forwarding).

## Setup

1. **On the host:** install usbip-win2 (the installer's optional **Steam Deck and DualSense USB audio and haptics** component, or its own release). Turn on **Settings → General → USB devices → USB forwarding** (`usb_forwarding`, off by default). **USB devices at once** (`usb_max_devices`, 1 to 16, default 4) limits how many devices this PC takes from all clients at the same time.
2. **For each client:** open the device under **Devices** in the console and tick **USB devices** under its permissions. Taking the permission away disconnects a streaming device, which detaches its USB devices.
3. **On Nightjar for Windows or Linux:** turn on **USB forwarding (Experimental)** in the settings. During a stream, the stream controls list the USB devices plugged in here (hubs left out), each with Attach or the reason it can't be forwarded.

The host forwards nothing while the setting is off, usbip-win2 is missing or the client lacks the permission; the client then says why.

## What can be forwarded

- **Windows clients** reach a device only through the WinUSB driver. A device under another driver (USBSTOR for a flash drive, HidUsb for a gamepad, an interface of a composite device) is listed with that driver named and Attach disabled. To forward it, install WinUSB for that device, for example with Zadig; Windows on the client can't use it while WinUSB has it. Device Manager's **Update driver** or `pnputil` gives it its own driver back.
- **Linux clients** need permission to open the device (`/dev/bus/usb`, usually a udev rule). Nightjar detaches the kernel driver while the device is attached, and Linux takes it back afterwards.
- Devices with isochronous endpoints (USB audio, webcams) are not forwarded.

## On the host

The console's **Devices** page shows a **USB devices (Experimental)** panel once a device has the permission or something is attached: each client's devices with their ID (`vid:pid`), bus ID and state (waiting for the device, connecting, attached on a port, detaching, detach failed). **Detach** unplugs one; **Detach again** retries a failed detach.

An attachment ends when the client detaches it, the console detaches it, the stream ends, either side closes the tunnel or the client disconnects.

## Troubleshooting

- **"Falscher Parameter" or "The parameter is incorrect" from `usbip detach`:** when the tunnel closes first, usbip-win2 unplugs the device on its own, and a later `usbip.exe detach --port <n>` fails with Windows' error in the display language. The host then asks `usbip.exe port <n>`; its answer `requested port(s) not found` (the same in every language) means the device is gone, and the detach counts as done. The host log says `usbip.exe detach failed for a port the driver already unplugged`. Any other answer keeps the failure, and the console shows **Detach failed** with the reason.
- **A device left attached after a crash:** `usbip.exe port` lists the host's ports, and `usbip.exe detach --port <n>` unplugs one.
