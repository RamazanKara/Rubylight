# DualSense waveform haptics

Rubylight can expose a composite DualSense controller on the Windows host, including its USB audio endpoint. Games write their native haptic audio to that endpoint; Rubylight forwards the two actuator channels using Nonary's Moonlight PCM extension. Ordinary rumble, adaptive triggers, lightbar, motion, touchpad and battery reports remain available.

This is experimental support in development builds from main. Released installers prepared before this change do not include it. Protocol, descriptor and queue checks run without installing a driver. Native game detection and physical controller output have not yet been verified end to end.

## Setup

1. In the Windows installer, select **Install USB audio and haptics support** on the **Steam Deck and DualSense USB audio and haptics** page. Development packages now bundle the signed usbip-win2 0.9.8.1 installer; it is optional and off by default. Setup remembers your choice for updates and reuses an existing shared USB/IP installation. Installing it can reconnect USB devices and require a Windows restart; setup never restarts Windows automatically. For an older Rubylight package, install [usbip-win2](https://github.com/vadimgrn/usbip-win2/releases) separately.
2. Use a client that implements Nonary's PCM haptics extension. For [Rubylight Android](https://github.com/RamazanKara/rubylight-android/blob/main/docs/dualsense-haptics.md), connect a DualSense or DualSense Edge by USB and enable both USB DualSense options in Advanced settings before reconnecting.
3. Leave **Settings → Input → Controller type** on **Automatic**, or choose **DualSense with waveform haptics (USB/IP)**. The same choice is available in an app's controller override.
4. Connect to Desktop first, wait for the controller and its audio device to appear on the host, then start the game. Disable host-side Steam Input for games that need to access the native DualSense. Games that only detect devices at startup need restarting after the controller connects.

Automatic selects the composite controller only when the chosen profile is DualSense and the client controller advertises waveform playback. If USB/IP is unavailable or fails to attach, Automatic keeps the VHF controller and shows a warning; native waveform haptics are unavailable in that fallback. The explicit USB/IP choice requires usbip-win2 and reports an error if it cannot attach.

The host exposes Sony USB identity 054c:0ce6 with HID and audio interfaces in the same device. Rubylight does not request a change to the Windows default audio device. If Windows selects the newly attached controller, switch game audio back to your normal output: the DualSense endpoint is for games' dedicated haptic output. Controller speaker/headset output and microphone capture are not carried by this extension; the virtual microphone returns silence.

Disconnecting or removing a controller detaches its virtual USB device and drops queued PCM. A reconnect creates the endpoint again; restart games that cached the old endpoint. Rubylight does not currently retain this USB controller across stream disconnects or wait for controller enumeration before automatically launching an app.

## Installer and portable packages

Quiet setup accepts `--usbip-driver` to select the component or `--no-usbip-driver` to skip it. Without either flag, setup and in-app updates keep the previously saved choice; a new installation leaves it off. Skipping the component does not remove an installed driver. If the shared USB/IP installation is incomplete, setup reports that it needs a restart or separate repair and leaves it unchanged.

The portable ZIP also carries the original installer at `drivers/usbip/USBip-0.9.8.1-x64.exe`; extracting the ZIP or running the portable host does not install it. Run it as administrator when you are ready. The host finds USBip through its registered install folder, the USBip folder under Program Files, or PATH.

USBip is shared with other applications. Rubylight uninstall, including `--remove-drivers`, leaves it installed. Remove **USBip** separately in Windows Settings if you no longer need it.

## Compatibility

- Both connection feature 0x04 and controller capability 0x8000 are required before waveform forwarding.
- PCM travels only over encrypted control, on ENet channel 8, as unreliable packets with a per-controller sequence. Clients without that channel or either capability receive ordinary feedback only.
- Packet type 0x5601 contains a 12-byte little-endian header: version 1, format 1, controller index, sequence, frame count and zero reserved bytes. Each full packet contains 240 stereo S16LE frames at 48 kHz (5 ms).
- USB audio channels 1–2 are discarded; channels 3–4 are preserved byte for byte. A separate queue retains at most 20 ms per controller, drops stale samples and never evicts reliable rumble, light or trigger feedback.
- The host vibration setting and the client's haptics toggle also apply to waveform packets.

The reference implementation is [Vibepollo 8b3ea1c4](https://github.com/Nonary/Vibepollo/tree/8b3ea1c4a41c383752475ffa551168a16b024d48), used by nightly 2.0.1-daily.d20261012.2. USB report and calibration data come from [libvirtualgamepad 9edbce11](https://github.com/Nonary/libvirtualgamepad/tree/9edbce11ad71e9455867c356c8368a6b39fa2242); composite descriptor credits and licenses are retained in the source and [third-party notices](../rust/THIRD_PARTY.md).
