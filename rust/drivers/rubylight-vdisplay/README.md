# Rubylight virtual display driver

`rubylight-vdisplay` is a UMDF IddCx virtual display driver for Rubylight, based on
[punktfunk](https://github.com/unom/punktfunk)'s `pf-vdisplay` by Enrico Bühler (unom),
MIT OR Apache-2.0 (see `LICENSE-MIT`, `LICENSE-APACHE`). It creates virtual monitors on the
Rubylight host's request and pushes every frame Windows composes for them to the host
through shared GPU textures, which the host converts and encodes directly.

It is the host's only virtual display driver: every display the host creates for a stream
comes from it, and the host reads those displays through its frame push. Setup installs it
with `install-local.ps1` (see [In the Rubylight package](#in-the-rubylight-package)) and
removes the libvirtualdisplay driver earlier versions used.

| | |
|---|---|
| Hardware ID | `Root\RubylightVDisplay` |
| UMDF service | `RubylightVDisplay` |
| Files | `rubylight_vdisplay.inf`, `rubylight_vdisplay.dll`, `rubylight_vdisplay.cat` |
| Control interface | `{5e240e6b-b196-4295-81c3-533c10000c59}`, IOCTL functions `0xB00`-`0xB0C` |

None of these collide with libvirtualdisplay (Vibepollo) or punktfunk, so an old
installation of either does not stand in its way.

## Layout

- `driver/`: the driver crate (cdylib).
- `wdk-iddcx/`: typed IddCx DDI wrappers (from punktfunk).
- `vendor/`: windows-drivers-rs 0.5.1 with punktfunk's IddCx bindings (see `vendor/README.md`).
- `../rubylight-vdisplay-proto/`: the host-driver contract, a member of the main workspace
  so CI tests it: IOCTLs and wire structs, the frame ring, mode lists and the EDID.
- `rubylight_vdisplay.inx`: the INF template.

This directory is its own cargo workspace (excluded from the main one): the driver builds
for `x86_64-pc-windows-msvc` with the static CRT and the Windows Driver Kit.

## How frames reach the host

1. The host creates the display with `IOCTL_ADD` and
   keeps it alive with `IOCTL_HEARTBEAT` every second. A monitor belongs to the process that
   created it and departs when that process closes its handles or goes silent for 10 s.
2. The capture backend `idd-push` creates, on the capture GPU's D3D12 device, four shared
   textures, two shared fences (*written* and *released*), a one-page header section and an
   event. It duplicates them into the driver's WUDFHost (its pid comes with the ADD reply) and
   hands them over with `IOCTL_SET_FENCE_RING`. The driver owns the handles if, and only if,
   that succeeds, and opens them on its own D3D12 device (`ID3D12Device::OpenSharedHandle`).
3. The driver renders on D3D12 through IddCx 1.11's D3D12 swap chain: it binds its device with
   `IddCxSwapChainSetDevice2` and receives each composed frame as `pD3D12Surface`. Every
   `IddCxSwapChainReleaseAndAcquireBuffer2` names the one queue that reads the surface (a copy
   queue per swap chain, at high priority; WUDFHost may not raise it to global realtime), so
   the OS orders its writes to the surface before the driver's read, and nothing else is
   submitted there.
4. Each texture has a state word in the header, changed only by compare-and-swap: free (with
   the released-fence value its last read ends at), writing, published or reading. For every
   frame the driver takes a free texture whose value the released fence has reached
   (`ID3D12Fence::GetCompletedValue`, never a wait), or reclaims the oldest published one the
   host has not claimed. It copies the surface into it on the copy queue and signals the
   written fence with the frame's sequence on the same queue, writes the sequence and the
   frame's timing into the header, marks the texture published, publishes the slot and sets
   the event. It never waits on the host or the GPU; with no texture to take, or every command
   list of the copy queue still on the GPU, it drops the frame.
5. The host claims the newest published slot, moving it to reading. Its compute queue waits for
   the written fence on the GPU and converts the texture in place. Several streams' conversions
   or a readback can read one frame: after the last of those reads the host signals the
   released fence on the same queue and frees the slot with that value. With
   `idd_push_zero_copy = false` a host compute queue instead waits for the written fence,
   copies the texture into the host's own pool and signals the released fence behind the
   copy, so the slot is free again at once. The image's present stamp is the driver's acquire: IddCx's `PresentDisplayQPCTime`
   is the scan-out, about 6.8 ms later on a 120 Hz display.

A host before `CAP_FENCE_RING` sends the older keyed-mutex ring (`IOCTL_SET_FRAME_RING`), which
this driver refuses before adopting any handle (`STATUS_NOT_SUPPORTED`); the driver never
reports `CAP_FRAME_RING`, and that host captures the display through Windows Graphics Capture
instead.

The driver loads only where the OS reports the D3D12 swap chain: `DriverEntry` logs the IddCx
framework version and `IddCxCheckOsFeatureSupport`'s answer, and fails with
`STATUS_NOT_SUPPORTED` without it (`IddMinimumVersionRequired` is 11). Setup then reports that
Windows did not start the driver.

With four textures the host holds at most two (the newest, and the one being encoded), so the
driver always finds one free; `idd_push=debug` logs its counters every 5 s (acquired, pushed,
dropped, overwritten) and `idd_push=trace` each frame's driver time (acquire to publish) and
handoff (publish to the host taking it).

The header also carries the driver's status (size or format mismatch, open failure, wrong
GPU, device lost), the surface it saw and a heartbeat of its swap-chain loop.

## Build

Prerequisites (all installed on the development PC on 2026-10-15):

- Visual Studio 2022 Build Tools with `Microsoft.VisualStudio.Component.VC.Tools.x86.x64`
- Windows Driver Kit 10.0.26100 matching the installed SDK
  (`winget install Microsoft.WindowsWDK.10.0.26100`). IddCx 1.11, which that kit lacks, comes
  from the pinned WDK 10.0.28000.2526 NuGet package: `driver-env.ps1` runs `fetch-iddcx.ps1`,
  which downloads it once (SHA-256 checked) and keeps only IddCx's headers and
  `IddCxStub.lib` in `target\iddcx-1.11`.
- LLVM for bindgen (`winget install LLVM.LLVM`)
- `rustup toolchain install 1.98.1-x86_64-pc-windows-msvc` (pinned by `rust-toolchain.toml`)

```powershell
cd rust\drivers\rubylight-vdisplay
.\build-package.ps1 -TestSign
```

The package lands in `target\package\`. `-TestSign` signs the DLL and the catalog with a
throwaway self-signed certificate it creates in memory and stores only as files in
`target\signing\` (`rubylight-vdisplay-test.pfx`, `.cer`, `.password`). It is never imported
into a certificate store. `cargo clippy` and `cargo fmt` work in this directory after
`. .\driver-env.ps1`.

## Installing for a local test (needs the machine owner's consent)

UMDF drivers need no kernel-mode driver signing, so **test-signing mode and Secure Boot stay
untouched**. Two checks still apply:

- Plug and Play verifies the package's catalog and installs it silently only when the signing
  certificate is in the local machine's Trusted Publishers store and chains to a trusted root.
  A self-signed certificate is its own root, so it goes into both `Root` and `TrustedPublisher`.
- The DLL must not be linked with `/INTEGRITYCHECK` (`IMAGE_DLLCHARACTERISTICS_FORCE_INTEGRITY`).
  windows-drivers-rs sets it for every driver; with it, WUDFHost has the kernel check the
  DLL's own signature, and the kernel trusts only Microsoft's roots. The device then fails
  with code 31: UMDF log event 2007 reports `0x80070241` (ERROR_INVALID_IMAGE_HASH) and
  CodeIntegrity/Operational logs 3004 plus 3089 with `VerificationError 7` (invalid root),
  even after a reboot with the certificate in `Root`. SudoVDA and the Sunshine driver are
  linked without it and load. The vendored `wdk-build` keeps the flag for kernel drivers
  only, and `build-package.ps1` refuses a DLL that has it.

References: [Trusted Publishers certificate store](https://learn.microsoft.com/windows-hardware/drivers/install/trusted-publishers-certificate-store),
[Loading test-signed code](https://learn.microsoft.com/windows-hardware/drivers/install/the-testsigning-boot-configuration-option)
(TESTSIGNING concerns kernel-mode drivers),
[Do UMDF drivers require signing?](https://learn.microsoft.com/archive/blogs/peterwie/do-umdf-drivers-require-signing),
[PnPUtil](https://learn.microsoft.com/windows-hardware/drivers/devtest/pnputil-command-syntax).

From an elevated PowerShell in this directory:

```powershell
$cer = "target\signing\rubylight-vdisplay-test.cer"
$thumb = (Get-PfxCertificate $cer).Thumbprint
certutil -addstore Root $cer
certutil -addstore TrustedPublisher $cer
pnputil /add-driver target\package\rubylight_vdisplay.inf
& "${env:ProgramFiles(x86)}\Windows Kits\10\Tools\10.0.26100.0\x64\devcon.exe" install target\package\rubylight_vdisplay.inf Root\RubylightVDisplay
pnputil /enum-devices /deviceid "Root\RubylightVDisplay" /drivers
```

`pnputil` has no command that creates a root-enumerated device, so `devcon install` creates
the `Root\RubylightVDisplay` devnode and binds the driver. The host needs no setting: it
captures every display it creates through the frame push, and has no other capture method.
A stream whose frame push cannot open fails to start with the reason; one that loses it
mid-stream (device loss, a display recreated) reopens it on the new device.

If the device shows code 31, enable the UMDF log
(`wevtutil sl Microsoft-Windows-DriverFrameworks-UserMode/Operational /e:true`), restart the
device (`pnputil /restart-device <instance id>`) and read that log and
`Microsoft-Windows-CodeIntegrity/Operational`. `setx /M RLVD_DEBUG_LOG 1` makes the driver
itself log to `rlvd-driver.log` in LocalService's temp folder once it loads.

The trusted test certificate lets anyone holding `rubylight-vdisplay-test.pfx` sign code
this machine trusts, so remove it when the test is over.

## Uninstalling

```powershell
& "${env:ProgramFiles(x86)}\Windows Kits\10\Tools\10.0.26100.0\x64\devcon.exe" remove Root\RubylightVDisplay
# or: pnputil /remove-device /deviceid "Root\RubylightVDisplay"
pnputil /enum-drivers   # find the oemNN.inf whose original name is rubylight_vdisplay.inf
pnputil /delete-driver oemNN.inf /uninstall /force
certutil -delstore TrustedPublisher $thumb
certutil -delstore Root $thumb
```

The host then has no virtual displays. The driver keeps
no state of its own beyond what its INF installs, which `pnputil /delete-driver /uninstall`
removes; its optional debug log (`RLVD_DEBUG_LOG`) is
`C:\Windows\ServiceProfiles\LocalService\AppData\Local\Temp\rlvd-driver.log`.

## Shipping before SignPath signs it: signed on the user's PC

`install-local.ps1` installs an **unsigned** package (`build-package.ps1` without
`-TestSign`) on the PC it runs on, elevated:

1. It creates a self-signed code-signing certificate with a non-exportable key in
   `LocalMachine\My`, signs the package's catalog with it, adds the certificate to `Root` and
   `TrustedPublisher`, and deletes the key, whatever happens.
2. It installs the package (`pnputil /add-driver /install`), creating the
   `Root\RubylightVDisplay` device with the bundled `nefconc.exe` when there is none.
3. It records the certificate's thumbprint in `HKLM\SOFTWARE\Rubylight\VirtualDisplay`
   (`LocalSigner`), deletes older packages and removes the previous install's certificate.
   `-Uninstall` removes the device, every package and the certificate.

No test-signing mode, no Secure Boot change, no reboot, no prompt. It works because a UMDF
driver needs no Microsoft signature: Plug and Play accepts a catalog that chains to a root in
`Root` with its signer in `TrustedPublisher`, and WUDFHost loads a DLL without an embedded
signature once it is linked without `/INTEGRITYCHECK` (see above). The installer already
does the same for the bundled Sunshine driver with that project's certificate, whose key its
maintainers hold. Here no key outlives the install, so the trusted certificate can sign
nothing else, for Rubylight or anyone who compromises it; every PC trusts only its own.

Verified on October 15 on the development PC (Windows 26200, Memory Integrity on): the
unsigned 0.1.0.6913 package installed silently, the earlier test certificate was removed,
the device restarted with status OK and no CodeIntegrity events, and an idd-push stream ran
at 2560x1440 and 240 Hz. Not covered: Windows in S mode or with Smart App Control on (both
refuse unknown signers, as they refuse the Sunshine driver), and WDAC-managed PCs.

### In the Rubylight package

Rubylight's only virtual display driver. `rust/build.ps1 -Package` builds the package with
`build-package.ps1` (unsigned), stamped with the release version and the build hour (2.3.1.7020,
say) so it outranks the 0.1.0.x development builds, and puts it under
`drivers\rubylight-display` with `install-local.ps1`, the bundled `nefconc.exe` and setup's
`remove-libvirtualdisplay.ps1`:

- Setup and an in-app update run `install-local.ps1 -InstallerBestEffort` as SYSTEM unless
  setup ran with `--no-display-driver`. A failure becomes a setup note, like the gamepad
  driver's.
- Once Rubylight's device runs (no note, no restart pending), they also run
  `remove-libvirtualdisplay.ps1 -InstallerBestEffort`, which removes the libvirtualdisplay
  device, driver packages and HDR layer earlier versions installed (a note when it cannot; the
  SignPath certificate stays, the gamepad driver needs it). Where the new driver was refused or
  failed, the old one stays; nothing uses it.
- When the host has had no virtual display driver for a minute after the service starts, the
  service runs `butterpollo-setup.exe --repair-drivers`, which installs it again: at most once
  a day per version, never during a stream. Nothing installs or removes it after a settings
  change.
- The uninstaller always runs `-Uninstall`, also when the other drivers stay.

`install-local.ps1` makes the device take the package it installs, even when a newer one is
in the driver store, and deletes the others before it removes their signer's trust. A
`pnputil` exit code of 3010 is a restart, not a failure.

Smart App Control, S mode and App Control (WDAC) policies refuse a driver signed on the PC.
`install-local.ps1` checks for them before it makes a certificate: it installs nothing,
removes an earlier install so no device that cannot start stays, and records why in
`HKLM\SOFTWARE\Rubylight\VirtualDisplay` (`Problem`), with what the user can do. Setup shows
that as its note and the console as the reason there is no virtual display; streams use a
physical display. A fresh install that fails, or whose device does not start, is rolled back
and recorded the same way. A SignPath-signed release installs on Smart App Control PCs.

A signed release replaces the catalog with the one SignPath signed
(`rust/release/sign-artifacts.ps1`). `install-local.ps1` then installs it as it is and adds
only that signer to `TrustedPublisher` (removed again by `-Uninstall` if it added it), and
deletes a locally signed install's packages and certificate.

## Release signing

A release package must be signed in this order, because the catalog hashes the signed DLL:
sign `rubylight_vdisplay.dll`, run Inf2Cat, sign `rubylight_vdisplay.cat`. With SignPath that
is two signing requests (or one per file type in a single artifact configuration that signs
the DLL before the catalog is generated). A package signed with a publicly trusted, non
Microsoft certificate still prompts unless its certificate is in Trusted Publishers; a silent
install on users' PCs needs Microsoft attestation signing (an EV certificate and Partner
Center) or the installer adding the publisher certificate to Trusted Publishers.
