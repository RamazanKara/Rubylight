# Virtual mouse and touchpad

[Documentation](README.md) · [Keyboard, touch, pen and cursor](configuration.md#keyboard-touch-pen-and-cursor) · [DualSense haptics](dualsense-haptics.md)

Rubylight can move the PC's mouse in two ways. **Injection** hands each move and click to Windows (`SendInput`). The **virtual mouse** plugs in a USB mouse over usbip-win2, the signed USB/IP driver the setup installs with **USB audio and haptics**, and sends it HID reports, so Windows, games and anti-cheat read it as they read a real mouse. The same driver carries a virtual **precision touchpad** that replays a device's physical touchpad, so Windows' own touchpad gestures work.

Neither device needs a new driver or signature: both are USB devices served by the host's USB/IP server (`butterpollo_core::usbip`), the path the virtual DualSense and Steam Deck controllers already use.

## Setup

1. Install **USB audio and haptics support** in the setup (optional, off by default; it can reconnect USB devices and may need a restart).
2. For a game that ignores injected input, set **Mouse input method** (`mouse_mode`) to **Virtual mouse**, which says in the stream when it cannot be used, or **Automatic**, which uses the virtual mouse when the driver is present and injection otherwise. The default, **Injection**, never plugs it in: measured here, the virtual mouse reached a game slightly later (p50 about 250 us against 190 us). A game or device can choose its own method under **Games** or **Devices**.
3. **Touchpad gestures from devices** (`native_touchpad`, on by default) offers devices the touchpad packets. Turn on **Touchpad gestures on the PC** in Nightjar; the touchpad's fingers then reach Windows' precision touchpad.

## What the devices are

| Device | USB id | Reports |
| --- | --- | --- |
| Mouse | `1209:52B1` | Report 1: five buttons, 16-bit relative X and Y, a wheel and horizontal scroll, each with a resolution multiplier (Windows sets 8 steps per notch, so fine scrolling stays fine both ways). Report 2: absolute X and Y (0-32767) on a single display. Interrupt endpoint polled every millisecond. |
| Precision touchpad | `1209:52B2` | Report 4: up to five contacts (confidence, tip, id, X and Y in 0.1 mm), scan time, contact count and the click. Feature reports: maximum contacts and pad type, the certification status Windows asks precision touchpads for, input mode and surface switch. |

The touchpad's size is the device's own when it reports one (105 x 70 mm otherwise). The touchpad is plugged in at a device's first touchpad packet and unplugged when the last stream using it ends.

Moves, clicks and scrolls keep their order with keys: each mouse event goes to the virtual mouse at the point where injection would have sent it. With several displays, absolute positions (desktop pointer, touch as a mouse) are still injected, because one absolute HID range cannot address a multi-monitor desktop the way injection can.

## Does the virtual mouse beat injection?

| | Injection | Virtual mouse |
| --- | --- | --- |
| Raw input (`WM_INPUT`) | Arrives with no device handle (`hDevice` 0) | Arrives from the device `VID_1209&PID_52B1`, like a physical mouse |
| Low-level mouse hook | Flagged `LLMHF_INJECTED` | Not flagged |
| Games and anti-cheat that drop injected input | Ignore it | Read it |
| Path | Straight into Windows' input queue | Report thread, loopback USB/IP, the usbip-win2 bus, the HID mouse stack, Windows' input queue |
| Needs | Nothing | usbip-win2 |
| Multi-display absolute positions | Yes | No (injection is used) |

The first three rows follow from how Windows marks input and are what the probe below checks on a given PC. The cost of the longer path is not assumed: it is measured with the probe, and no figure is claimed here until it is.

**Can injection be removed?** No. The virtual mouse needs usbip-win2, which stays optional; it cannot place absolute positions on a multi-display desktop; and when it cannot be plugged in or goes away mid-stream, input must keep working. Injection stays as the fallback, with a log line, and with a stream message when the setting asked for the virtual mouse.

## Measuring it

`input_latency_probe` (an example of the `butterpollo-windows` crate) shows what a game sees:

```powershell
cargo run --release -p butterpollo-windows --example input_latency_probe -- watch 30 vm.csv
```

It takes raw mouse input in the background as games do and installs a low-level mouse hook, writes every `WM_INPUT` with its QueryPerformanceCounter time, sending device and motion, and every hook event with its injected flag, then prints the devices seen and the injected share. Run it on the host PC in the user's session while a client moves the mouse in short strokes.

For the delay, run the host with `RUST_LOG=info,input_timing=trace`: each relative move is logged with its QPC time as it reaches the injector (`arrived`), and each delivery with its route (`delivered`, `virtual_mouse` or `injection`). Then:

```powershell
cargo run --release -p butterpollo-windows --example input_latency_probe -- pair butterpollo.log vm.csv
```

pairs each `WM_INPUT` with the latest earlier arrival of the same motion delivered on its route (injected `WM_INPUT` with `injection`, the virtual mouse's with `virtual_mouse`), so a move that never reached the probe stays unpaired instead of shifting later pairs, and prints p50, p95 and p99 per device. Compare one run with `mouse_mode = "virtual_mouse"` and one with `mouse_mode = "injection"`, 30-60 s each, on an isolated test host.

