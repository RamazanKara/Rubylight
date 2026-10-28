# DualSense waveform haptics

Rubylight can expose a composite DualSense controller on the Windows host, including its USB audio endpoint. Games write their native haptic audio to that endpoint; Rubylight forwards the two actuator channels using Nonary's Moonlight PCM extension. Ordinary rumble, adaptive triggers, lightbar, motion, touchpad and battery reports remain available.

Waveform haptics and the bundled USB/IP installer are available in Rubylight 2.2.15 and later. Controller speaker/headset audio, microphone input and controller preparation/continuity are available in Rubylight 2.2.16; the additional client audio paths require a matching client such as Nightjar. A client that only implements Nonary's waveform extension can still receive haptics.

## Setup

1. In the Windows installer, select **Install USB audio and haptics support** on the **Steam Deck and DualSense USB audio and haptics** page. Rubylight 2.2.15 and later bundle the signed usbip-win2 0.9.8.1 installer; it is optional and off by default. Setup remembers your choice for updates and reuses an existing shared USB/IP installation. Installing it can reconnect USB devices and require a Windows restart; setup never restarts Windows automatically. For an older Rubylight package, install [usbip-win2](https://github.com/vadimgrn/usbip-win2/releases) separately.
2. Use a client that implements Nonary's PCM haptics extension. For [Nightjar](https://github.com/RamazanKara/nightjar/blob/main/docs/dualsense-haptics.md), connect a DualSense or DualSense Edge by USB and enable **USB DualSense driver** and **USB DualSense audio and haptics** under **Settings → Controls** before reconnecting.
3. Leave **Settings → Input → Controller type** on **Automatic**, or choose **DualSense with waveform haptics (USB/IP)**. The same choice is available in an app's controller override.
4. With Nightjar, controllers that are ready when the connection starts are attached before Rubylight launches the game. With other clients, connect to Desktop first and wait for the controller/audio device before starting the game. Disable host-side Steam Input for games that need native DualSense effects. For Steam gyro aiming and radial menus, enable Steam Input and configure those in the game’s controller layout; Rubylight forwards the controller motion and touchpad input.

Automatic selects the composite controller only when the chosen profile is DualSense and the client controller advertises waveform playback. If USB/IP is unavailable or fails to attach, Automatic keeps the VHF controller and shows a warning; native waveform haptics are unavailable in that fallback. The explicit USB/IP choice requires usbip-win2 and reports an error if it cannot attach.

The host exposes Sony USB identity 054c:0ce6 with HID and audio interfaces in the same device. Rubylight does not request a change to the Windows default audio device. If Windows selects the newly attached controller, switch game audio back to your normal output: the DualSense endpoint is for games' dedicated haptic output. Matching updated Rubylight host/client builds also carry controller speaker sound, headphone audio and microphone input alongside waveform haptics. These additions are separate from the Vibepollo waveform extension: clients implementing only that extension still receive haptics, with a silent virtual microphone.

A stream disconnect releases held controls and clears buffered audio while retaining virtual controller identities for the same paired client and stream role. Reconnects reuse compatible controllers and their audio endpoints. Devices expire only when no game is running and they have been without a transport for two minutes. A physical controller removal during a stream still detaches that controller. Changing the controller profile or capabilities can recreate it. Nightjar can automatically resume the same running game after eligible interruptions, with bounded retries and manual reconnect after recovery stops; see its [recovery policy](https://github.com/RamazanKara/nightjar/blob/main/docs/recovery-and-quality.md#recovering-a-connection).

During an active connection, the host keeps the latest controller audio settings through slow USB attachment and temporary input release. Disconnect or controller removal clears that transport's audio state and buffered samples; reconnects apply fresh client state.

Microphone input requires the paired device's **Controllers** permission, the host microphone setting, Android’s microphone setting and recording permission. The stream mute control and DualSense mute button are synchronized; Android’s global microphone mute and host mute commands also stop capture. Nightjar keeps the stream's mute state across reconnects and lock/resume. Select the virtual DualSense microphone in the game or voice application. Plugging a headset into the physical controller routes stream sound there, including a stereo mix of surround streams; unplugging it restores Android’s usual output. With several controllers, the lowest numbered attached headset receives normal stream sound. Rubylight does not change Windows audio defaults.

The host acknowledges controller microphone support only after its USB audio endpoint is ready. Android switches from its ordinary microphone only after that acknowledgement and successful USB capture startup. During attachment, or when Automatic falls back to VHF, the ordinary microphone route remains available subject to the same permission and mute controls. Disconnecting a host-muted controller keeps the fallback muted until you unmute it in the stream menu. The client refreshes controller status once per second, so successful USB attachment is acknowledged on the next refresh; normal stream sound to a connected headset remains available independently. See the [Android audio and fallback guide](https://github.com/RamazanKara/nightjar/blob/main/docs/dualsense-haptics.md) for client setup and limits.

With [Game-only audio](configuration.md#game-only-audio), dedicated USB speaker PCM is suppressed because the virtual endpoint cannot identify which application wrote it. Normal process-captured game sound still reaches a connected controller headset. Actuator haptics and microphone permissions remain independent. Disabled stream audio and muted Remote Monitor sessions also suppress this separate speaker path.

## Haptics from game audio

<a id="haptics-from-game-audio-experimental"></a>

Most games send a DualSense rumble at best and often nothing. With **Settings → Input → Controllers → Haptics from game audio** (`audio_haptics`, off by default) the host makes waveform haptics from the stream's own sound for each client DualSense that has waveform haptics but gets none from the game:

- The bass below 150 Hz drives the left and right actuators from the left and right channels; in 5.1 and 7.1 the LFE drives both, and the rear and side channels add to their side.
- Sharp transients such as shots and impacts add a short 100 Hz thump on their side, as strong as the transient, even when they carry no bass.
- **Audio haptics strength** (`audio_haptics_strength`, 0 to 200 %, default 100) scales the result. **Audio haptics response** (`audio_haptics_curve`) is **Linear** (the default), **Soft** (lifts quiet bass) or **Punchy** (keeps only big hits). **Audio haptics noise gate** (`audio_haptics_gate`, -80 to -10 dBFS, default -50) keeps quieter bass off the actuators. All four apply mid-stream when saved, globally or in an app's overrides; a per-device override applies from the client's next stream.
- It stops the moment the game sends that controller its own waveform haptics, rumble or trigger rumble, and resumes a minute after the game's last. Their 0x5601 sequence numbers continue from each other, so the client never discards the game's frames.
- On Nightjar the hand-over is not instant on the controller: up to 20 ms of generated samples already queued still play, and the client's haptics queue stays in waveform mode for 100 ms after the last audible frame, so a game's rumble that follows generated haptics can start up to that late. Not yet checked on a device.
- The analysis runs on the control thread from a copy of the audio already sent; the audio path only copies a packet into a lock-free ring while a controller wants it. Audio gains no latency, and silent stretches send no haptics packets.
- It needs what waveform forwarding needs from the client (connection feature 0x04, capability 0x8000, encrypted control with channel 8), with **Vibration** on and the client's haptics toggle on. USB/IP is not needed: the frames do not pass through the virtual controller, so any controller profile works.
- A value outside these ranges in a hand-edited configuration turns the feature off for the stream with a warning on the stream card.

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
