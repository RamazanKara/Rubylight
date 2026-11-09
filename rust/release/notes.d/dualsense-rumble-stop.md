# DualSense rumble stop
- **DualSense vibration stops when Steam stops it.** With the DualSense controller that has waveform haptics (usbip-win2 installed), Steam Input's Identify Controller and other Steam rumble no longer keeps the device vibrating until the stream loses focus.
- Known issue: with the DualSense from the virtual gamepad driver (without usbip-win2), Steam's rumble stop can still be lost, because the driver reports it as the previous rumble. Until the driver is fixed, install usbip-win2 or choose DualShock 4 emulation.
