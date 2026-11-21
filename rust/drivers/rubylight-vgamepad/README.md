# rubylight-vgamepad

Rubylight's first-party UMDF 2 user-mode HID (VHF) virtual gamepad driver, written in Rust against `windows-drivers-rs` (`wdk-sys` / `wdk-build`) and `rubylight-vgamepad-proto`.

Derived from [`Nonary/libvirtualgamepad`](https://github.com/Nonary/libvirtualgamepad) by Chase Payne (MIT license), including `jlobue10`'s Steam Controller (2026) profile (PR #6).

## Scope and parity

`rubylight-vgamepad` provides full gamepad parity with upstream `Nonary/libvirtualgamepad`:

- **Controller profiles**: Xbox One, Xbox Series X|S (including Guide + Share), DualShock 4 (v1/v2 touch + gyro/accel + lightbar + rumble + calibration), DualSense (touch + gyro/accel + LED/player lights + trigger effects + firmware info + calibration + BT output bridge), Switch Pro (USB + HID handshake, SPI flash calibration, IMU, HD rumble), and Steam Controller 2026 (handshake replay, explicit latches, settings & haptic queues, 4 ms high-res keepalive timer, battery period); `rubylight-vgamepad-proto` also retains the Generic HID and Generic PID force-feedback engine definitions from upstream.
- **Lifecycle & transport**: 16 concurrent slots per VHF device, Configuration Manager child instance collision detection (`CM_Get_Device_ID_ListW`), owner cleanup on handle close, D0 exit loss marking (`STATUS_DEVICE_REMOVED`) so hosts cleanly recreate controllers after power transitions, and three-tier input report pacing (`ReportPump`).
- **Side-by-side safe identity**: Uses Rubylight's own device interface GUID, hardware ID (`Root\RubylightVirtualGamepad`), INF (`rubylight_vgamepad.inf`), and DLL (`rubylight_vgamepad.dll`), so it never collides with Vibepollo or Sunshine installations.

### Deliberate exclusions

- **Optional keyboard/mouse VHF source (`0x820..0x828`) and unsigned KMDF `mouse_mapper`**: Intentionally excluded. Rubylight provides first-party USB/IP virtual mouse and precision touchpad devices and does not load an unsigned kernel-mode driver.

## Building and packaging

Requires the MSVC Rust toolchain (`rust-toolchain.toml`), Visual Studio C++ x64 build tools, LLVM (`libclang.dll`), and Windows Driver Kit (`10.0.26100.0` or newer, providing `VhfUm.lib`, `cfgmgr32.lib`, `stampinf.exe`, `Inf2Cat.exe`, `signtool.exe`):

```powershell
.\rust\drivers\rubylight-vgamepad\build-package.ps1 -TestSign
```

`-TestSign` signs the package with a throwaway test certificate under `target\signing` without adding anything to Windows certificate stores.

## Local installation

Elevated `install-local.ps1` (run automatically by `butterpollo-setup.exe`) installs the driver package onto `Root\RubylightVirtualGamepad`. See `rust/drivers/rubylight-vdisplay/README.md` for details on self-signed local catalog trust vs release Authenticode catalogs.
