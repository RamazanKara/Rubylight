# DualSense waveform haptics

[Controls](controls.md#dualsense-and-usb) · [Settings](settings.md)

Rubylight Android implements the waveform extension used by [Nonary's Vibepollo nightly](https://github.com/Nonary/Vibepollo/releases/tag/2.0.1-daily.d20261012.2). A USB-connected DualSense or DualSense Edge can receive the game's distinct left/right actuator samples alongside input, adaptive triggers and lights. This is experimental: automated protocol and sample-handling checks do not establish physical feedback quality on Android devices.

## Setup

1. Enable **Settings → Advanced → USB DualSense driver** and **USB DualSense waveform haptics** on Android. Both are off by default.
2. Connect the controller by USB, grant Android's USB access request and start a new stream. Reconnect the controller and stream after changing the option.
3. On a Windows Vibepollo host, install its optional **DualSense USB audio and haptics** component and select a DualSense controller profile. Follow [Vibepollo's host instructions](https://github.com/Nonary/Vibepollo/blob/8b3ea1c4a41c383752475ffa551168a16b024d48/docs/windows-dualsense-haptics.md). Linux hosts must likewise expose a waveform-capable DualSense backend.
4. Enable native DualSense support in a compatible game. Steam Input translation can prevent the game from seeing the native controller and its audio endpoint.

The Android log reports `DualSense USB waveform backend ready` only after a silent USB audio transfer succeeds. If the phone's USB implementation cannot provide that output, Rubylight keeps ordinary controller feedback and does not advertise waveform support. Reconnect to retry after an output failure.

## Limitations

- **USB only.** Android's standard Bluetooth controller path does not expose raw waveform output. This option does not convert the samples into phone vibration or ordinary rumble.
- **Controller audio is reserved for haptics.** The controller's headphone/speaker channels stay silent, and its microphone/headset interfaces are unavailable while the option owns USB audio. Use another audio device for the game. Disable the option and reconnect the controller to restore its normal USB audio use.
- **Host and game support are required.** Advertising the extension does not generate effects on a host or in a game that does not send waveform samples.
- **Physical validation remains needed.** Check distinct left/right effects on a DualSense and Edge, simultaneous triggers/lights/input, recovery to ordinary rumble, stream teardown and controller reconnect on the target Android device.

## Protocol and implementation

The encrypted control packet is `0x5601`; connection feature `ML_FF_HAPTICS_PCM` is `0x04`, and per-controller capability `LI_CCAP_HAPTICS_PCM` is `0x8000`. The version-1 payload has a 12-byte little-endian header followed by 1–240 frames of 48 kHz stereo S16LE actuator PCM. Invalid version, format, controller, reserved fields and lengths are rejected before the Java callback.

Each ready controller owns a bounded queue and a separate USB connection. The worker copies actuator samples to channels 3/4 of four-channel USB audio, leaves channels 1/2 silent, drops duplicate/out-of-order and stale samples, and discards queued samples across sequence gaps. At most 20 ms of PCM waits in software and 8 ms is submitted to USB. HID output keeps waveform mode enabled during adaptive-trigger and light changes; sustained host silence returns control to ordinary rumble.

The protocol definition retains the GPL-3.0-or-later notice from [Nonary/moonlight-common-c](https://github.com/Nonary/moonlight-common-c/blob/4ea115e8d23d1ca055a2aa8ddcfb601b705b3323/src/ControllerHaptics.h). USB format validation follows the [physical DualSense descriptors published by nondebug](https://github.com/nondebug/dualsense/blob/main/lsusb-descriptor-info.txt). Existing HID reports retain their SDL attribution.
