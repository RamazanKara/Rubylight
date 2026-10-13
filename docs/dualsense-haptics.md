# DualSense waveform haptics

Rubylight can expose a composite DualSense controller on the Windows host, including its USB audio endpoint. Games write their native haptic audio to that endpoint; Rubylight forwards the two actuator channels using Nonary's Moonlight PCM extension. Ordinary rumble, adaptive triggers, lightbar, motion, touchpad and battery reports remain available.

Available in Rubylight 2.2.15 and later. Earlier releases do not include DualSense waveform haptics or the bundled USB/IP installer.

## Setup

1. In the Windows installer, select **Install USB audio and haptics support** on the **Steam Deck and DualSense USB audio and haptics** page. Rubylight 2.2.15 and later bundle the signed usbip-win2 0.9.8.1 installer; it is optional and off by default. Setup remembers your choice for updates and reuses an existing shared USB/IP installation. Installing it can reconnect USB devices and require a Windows restart; setup never restarts Windows automatically. For an older Rubylight package, install [usbip-win2](https://github.com/vadimgrn/usbip-win2/releases) separately.
2. Use a client that implements Nonary's PCM haptics extension. For [Rubylight Android](https://github.com/RamazanKara/rubylight-android/blob/main/docs/dualsense-haptics.md), connect a DualSense or DualSense Edge by USB and enable both USB DualSense options in Advanced settings before reconnecting.
3. Leave **Settings → Input → Controller type** on **Automatic**, or choose **DualSense with waveform haptics (USB/IP)**. The same choice is available in an app's controller override.
4. With the matching updated Rubylight Android client, controllers that are ready when the connection starts are attached before Rubylight launches the game. With other clients, connect to Desktop first and wait for the controller/audio device before starting the game. Disable host-side Steam Input for games that need native DualSense effects. For Steam gyro aiming and radial menus, enable Steam Input and configure those in the game’s controller layout; Rubylight forwards the controller motion and touchpad input.

Automatic selects the composite controller only when the chosen profile is DualSense and the client controller advertises waveform playback. If USB/IP is unavailable or fails to attach, Automatic keeps the VHF controller and shows a warning; native waveform haptics are unavailable in that fallback. The explicit USB/IP choice requires usbip-win2 and reports an error if it cannot attach.

The host exposes Sony USB identity 054c:0ce6 with HID and audio interfaces in the same device. Rubylight does not request a change to the Windows default audio device. If Windows selects the newly attached controller, switch game audio back to your normal output: the DualSense endpoint is for games' dedicated haptic output. Matching updated Rubylight host/client builds also carry controller speaker sound, headphone audio and microphone input alongside waveform haptics. These additions are separate from the Vibepollo waveform extension: clients implementing only that extension still receive haptics, with a silent virtual microphone.

A stream disconnect releases held controls and clears buffered audio while retaining virtual controller identities for the same paired client and stream role. Reconnects reuse compatible controllers and their audio endpoints. Devices remain parked while a game is running, then expire after two minutes without a transport. A physical controller removal during a stream still detaches that controller. Changing the controller profile or capabilities can recreate it. Android retries interrupted connections three times with bounded delays and offers manual reconnect after that.

Microphone input requires the host microphone setting, Android’s microphone setting and recording permission. The stream mute control and DualSense mute button are synchronized; Android’s global microphone mute and host mute commands also stop capture. Select the virtual DualSense microphone in the game or voice application. Plugging a headset into the physical controller routes stream sound there, including a stereo mix of surround streams; unplugging it restores Android’s usual output. With several controllers, the lowest numbered attached headset receives normal stream sound. Rubylight does not change Windows audio defaults.

## Installer and portable packages

Quiet setup accepts `--usbip-driver` to select the component or `--no-usbip-driver` to skip it. Without either flag, setup and in-app updates keep the previously saved choice; a new installation leaves it off. Skipping the component does not remove an installed driver. If the shared USB/IP installation is incomplete, setup reports that it needs a restart or separate repair and leaves it unchanged.

The portable ZIP also carries the original installer at `drivers/usbip/USBip-0.9.8.1-x64.exe`; extracting the ZIP or running the portable host does not install it. Run it as administrator when you are ready. The host finds USBip through its registered install folder, the USBip folder under Program Files, or PATH.

USBip is shared with other applications. Rubylight uninstall, including `--remove-drivers`, leaves it installed. Remove **USBip** separately in Windows Settings if you no longer need it.

## Compatibility

- Both connection feature 0x04 and controller capability 0x8000 are required before waveform forwarding.
- PCM travels only over encrypted control, on ENet channel 8, as unreliable packets with a per-controller sequence. Clients without that channel or either capability receive ordinary feedback only.
- Packet type 0x5601 contains a 12-byte little-endian header: version 1, format 1, controller index, sequence, frame count and zero reserved bytes. Each full packet contains 240 stereo S16LE frames at 48 kHz (5 ms).
- USB audio channels 3–4 remain dedicated to actuator PCM. With controller audio negotiated, channels 1–2 travel separately as speaker/headset PCM. Separate queues retain at most 20 ms, drop stale samples and never evict reliable rumble, light or trigger feedback.
- The host vibration setting and the client's haptics toggle also apply to waveform packets.

The reference implementation is [Vibepollo 8b3ea1c4](https://github.com/Nonary/Vibepollo/tree/8b3ea1c4a41c383752475ffa551168a16b024d48), used by nightly 2.0.1-daily.d20261012.2. USB report and calibration data come from [libvirtualgamepad 9edbce11](https://github.com/Nonary/libvirtualgamepad/tree/9edbce11ad71e9455867c356c8368a6b39fa2242); composite descriptor credits and licenses are retained in the source and [third-party notices](../rust/THIRD_PARTY.md).
