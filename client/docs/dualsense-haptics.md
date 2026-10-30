# DualSense USB audio and haptics

[Controls](controls.md#dualsense-and-usb) · [Settings](settings.md)

Nightjar implements the DualSense waveform extension that [Vibepollo](https://github.com/Nonary/Vibepollo) and Rubylight hosts send. A USB-connected DualSense or DualSense Edge can receive the game's distinct left/right actuator samples alongside input, adaptive triggers and lights. On hosts that support it (Rubylight), controller speaker, headset and microphone audio are carried too. Older releases and Vibepollo's waveform-only extension do not provide these additional audio paths.

## Setup

1. Enable **Settings → Controls → USB DualSense driver** and **USB DualSense audio and haptics** on Android. Both are off by default.
2. Connect the controller by USB, grant Android's USB access request and start a new stream. Reconnect the controller and stream after changing the option.
3. Enable DualSense haptics on the host. On a Windows Rubylight host, select the bundled **USB audio and haptics support** installer component and Automatic or USB/IP DualSense controller type. See [host setup](https://github.com/RamazanKara/Rubylight/blob/main/docs/dualsense-haptics.md). For waveform-only support with Vibepollo, follow [its host instructions](https://github.com/Nonary/Vibepollo/blob/8b3ea1c4a41c383752475ffa551168a16b024d48/docs/windows-dualsense-haptics.md).
4. Enable native DualSense support in a compatible game. Steam Input translation can prevent the game from seeing the native controller and its audio endpoint.
5. For controller microphone input, enable microphone streaming and grant Android's recording permission. Select the virtual DualSense microphone in the host game or voice application. The controller mute button and the stream menu share one mute setting; host mute commands and Android's global microphone mute also stop USB capture.

Plugging a headset into the controller sends normal stream sound to it, mixed with any controller-specific audio. Android's media volume controls normal stream sound. Surround streams are mixed to stereo for the headset. Unplugging the headset returns normal sound to Android's usual output. With multiple controllers, the lowest numbered connected headset receives normal stream sound.

Preparing a controller before launch does not change the microphone source. The ordinary Android microphone continues until the host accepts controller capture and the USB microphone opens. If that capture fails or the last controller disconnects, the ordinary microphone can resume only while recording remains authorized and unmuted. Host mute, the stream mute setting, Android's microphone mute and revoked input access prevent this fallback from starting capture. Disconnecting a host-muted controller keeps the fallback muted until you unmute it in the stream menu.

For gyro aiming and radial menus, enable controller motion in Nightjar and configure the game's Steam Input layout on the host. Steam provides [gyro behavior](https://partner.steamgames.com/doc/features/steam_controller/input_source_modes) and [radial menus](https://partner.steamgames.com/doc/features/steam_controller/radial_menus) using the motion and touchpad input Nightjar forwards. Native DualSense effects and Steam Input translation may require different per-game settings.

The Android log reports `DualSense USB waveform backend ready` only after a silent USB audio transfer succeeds. If the phone's USB implementation cannot provide that output, Nightjar keeps ordinary controller feedback and does not advertise waveform support. Reconnect to retry after an output failure.

## Troubleshooting missing feedback

After reproducing the problem, use **Settings → App → Report a problem** without closing the app. Include the phone and Android version, controller model, USB connection, exact host version and game. On Vibepollo, also check that the optional **DualSense USB audio and haptics** installer component is installed and the host presents a DualSense to the game. A client setting alone cannot enable the host's audio endpoint or a game's native effects.

The report includes USB HID readiness, the waveform initialization stage, queued controller capabilities and the first received waveform/adaptive-trigger packet in the report. These distinguish USB setup failures from feedback that never arrives from the host. A successful queue result confirms local submission, not that the game produced effects.

A pending USB controller's input waits for its type and capabilities to be queued; rejected startup announcements are retried. Opening the stream menu or unplugging that controller does not bypass this ordering. Buttons already held by an existing controller or on-screen controls can still release while the new controller's announcement is pending.

## Limitations

- **USB only.** Android's standard Bluetooth controller path does not expose raw waveform output. This option does not convert the samples into phone vibration or ordinary rumble.
- **Additional audio needs a host that supports it (Rubylight).** With a waveform-only host, use another audio device for sound and microphone input. Disable this option and reconnect the controller to restore Android's normal USB audio ownership.
- **Host and game support are required.** Advertising the extension does not generate effects on a host or in a game that does not send waveform samples.
- **Device compatibility.** The USB backend checks the controller's descriptors and transfer support. Unsupported USB configurations retain ordinary feedback. Automated checks cover packets and sample handling; physical sound, haptics and headset switching still need validation on the target devices.

## Reconnects

Interrupted streams use the [bounded, resume-only recovery policy](recovery-and-quality.md). Returning to an established game after screen lock uses the same policy and a fresh decoder; a stopped or replaced host application is never launched again automatically. Hosts that support it (currently Rubylight) retain controller identities and audio endpoints across reconnects, neutralize held controls, and prepare ready USB controllers before launching a game. A controller connected after launch may still require restarting games that only detect devices at startup.

## Protocol and implementation

The encrypted control packet is `0x5601`; connection feature `ML_FF_HAPTICS_PCM` is `0x04` and is offered only while this option is on, because it moves a Vibepollo host's control loop to a 5 ms period; per-controller capability `LI_CCAP_HAPTICS_PCM` is `0x8000`. The version-1 payload has a 12-byte little-endian header followed by 1–240 frames of 48 kHz stereo S16LE actuator PCM. Invalid version, format, controller, reserved fields and lengths are rejected before the Java callback.

Each ready controller owns separate bounded queues for actuator PCM and audible sound. The worker copies actuators to channels 3/4 and sound to channels 1/2 of four-channel USB audio, drops duplicate/out-of-order and stale samples, and discards queued samples across sequence gaps. At most 20 ms of each PCM stream waits in software and 8 ms is submitted to USB. Microphone capture uses its own USB connection so it cannot consume playback completions. Capture is registered with Android's app-operation tracking and stops on mute, permission loss or teardown. HID output keeps waveform mode enabled during adaptive-trigger, audio and light changes; sustained host silence returns control to ordinary rumble.

The Rubylight host's separately advertised encrypted control types are `0x5533` (speaker PCM), `0x5534` (microphone PCM), and `0x5535` (audio state). PCM uses the same bounded 12-byte header and 48 kHz stereo S16LE format. Audio state operation 1 is an eight-byte client capability/mute report; operation 3 acknowledges the host microphone policy. Their flags are playback=1, capture=2, headphones=4, muted=8. Operation 2 is a twelve-byte host HID audio-control update: version, operation, controller index, two valid-field masks, headphone/speaker/microphone volume, routing, microphone LED and mute flags. Audio controls are reliable; PCM is not retransmitted. This extension does not change Nonary's waveform protocol.

The protocol definition retains the GPL-3.0-or-later notice from [Nonary/moonlight-common-c](https://github.com/Nonary/moonlight-common-c/blob/4ea115e8d23d1ca055a2aa8ddcfb601b705b3323/src/ControllerHaptics.h). USB format validation follows the [physical DualSense descriptors published by nondebug](https://github.com/nondebug/dualsense/blob/main/lsusb-descriptor-info.txt). Existing HID reports retain their SDL attribution.
