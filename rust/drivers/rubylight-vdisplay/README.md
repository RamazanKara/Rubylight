# Rubylight virtual display driver

`rubylight-vdisplay` is a UMDF IddCx virtual display driver for Rubylight, based on
[punktfunk](https://github.com/unom/punktfunk)'s `pf-vdisplay` by Enrico Bühler (unom),
MIT OR Apache-2.0 (see `LICENSE-MIT`, `LICENSE-APACHE`). It creates virtual monitors on the
Rubylight host's request and pushes every frame Windows composes for them to the host
through shared GPU textures, so the host converts and encodes them without WGC or Desktop
Duplication.

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

The driver's keyed-mutex ring, which the host no longer uses (the [GPU fence transport](#gpu-fence-transport) replaces it), works like this:

1. The host creates the display with `IOCTL_ADD` and
   keeps it alive with `IOCTL_HEARTBEAT` every second. A monitor belongs to the process that
   created it and departs when that process closes its handles or goes silent for 10 s.
2. A host using it creates four `SHARED_NTHANDLE | SHARED_KEYEDMUTEX` textures
   on the GPU that renders the monitor, a one-page header section and an event, duplicates
   them into the driver's WUDFHost (its pid comes with the ADD reply) and hands them over
   with `IOCTL_SET_FRAME_RING`. The driver owns the handles if, and only if, that succeeds.
3. For every frame from `IddCxSwapChainReleaseAndAcquireBuffer2` the driver takes a texture
   whose keyed mutex it can get with key 0 (or reclaims the oldest unconsumed one with key 1),
   copies the surface into it, writes the sequence and `PresentDisplayQPCTime` into the header,
   releases with key 1, publishes the slot and sets the event. It never waits on the host.
4. The host takes the newest published slot with key 1. On AMD its compute conversion reads
   the texture in place: the host keeps the mutex until the conversion that read it is done,
   then releases with key 0 (`idd_push_zero_copy = false` copies instead). Elsewhere it
   copies the texture into its own pool and releases at once. The image's present stamp is
   the driver's acquire: IddCx's `PresentDisplayQPCTime` is the scan-out, about 6.8 ms later
   on a 120 Hz display.

Four textures let capture overlap the host's GPU reads. If every slot is busy, the driver
keeps the newest image in its own texture, copying it before reporting the IddCx frame
finished. When IddCx has no newer frame (`E_PENDING`), the driver retries that owned copy
without waiting for a slot. Failed retries wait 2, 4, 8, then at most 16 ms, interruptible
by a new frame, ring replacement or stop. A final desktop update can therefore reach the
host after a slot becomes free even if Windows stops presenting the static desktop.
Retries keep the original present/acquire timestamps and do not add acquisitions or
dropped frames to the counters. Success stops retries; a newer acquisition supersedes
the saved image. This applies to both transports. With no pending image that fits the
ring, the normal idle heartbeat wait remains 125 ms.

`idd_push=debug` logs the counters every 5 s (acquired, pushed, dropped, overwritten), and
`idd_push=trace` each frame's driver time (acquire to publish) and handoff (publish to the
host taking it).

The header also carries the driver's status (size or format mismatch, open failure, wrong
GPU, device lost), the surface it saw and a heartbeat of its swap-chain loop.

### Sleep and device recovery

The IddCx adapter and monitor identities survive a D0-to-Dx sleep transition. D0 exit
pauses the owner watchdog, drains control requests, and retires swap-chain workers, shared
frame rings and pooled rendering devices. Resume reuses the existing adapter and monitors,
rebases the watchdog grace period and lets fresh swap-chain assignments and capture rings
restore rendering. Only a real PnP stop (`EvtDeviceReleaseHardware`) discards the adapter
and framework-owned monitor records. Late initialization callbacks are matched against the
object returned by their initialization call, so an old callback cannot replace a new
adapter. This follows the [IddCx object lifetime](https://learn.microsoft.com/en-us/windows-hardware/drivers/display/iddcx-objects).
Initial adapter-creation errors still propagate to PnP, and failed asynchronous
initializations never expose an adapter as ready; only a healthy resume skips creation.

If a launch or retained lease cannot open the control interface, the host can request a
restart of the single installed `Root\RubylightVDisplay` devnode using the `WUDFRd`
reflector. It requires a stopped device with PnP failure code 10, 31 or 43. Healthy devices,
manually disabled devices, other display drivers, ambiguous duplicate installations and
devices requiring setup or reboot are left alone. Recovery uses one background worker,
at most three restart attempts with a 60-second cooldown, and a three-second caller wait.
Console/status checks do not trigger recovery. The host never installs a driver or requests
a Windows reboot through this path; an unelevated process may be unable to restart it.

The lifecycle and restart-selection policies have ordinary tests. Real sleep, hibernation,
PnP restart and multi-session capture still require separate hardware validation of the
installed driver; compiling the DLL does not perform those transitions.

### GPU fence transport

The fence ring is the only transport from this driver to the host. There is no setting for it
(`idd_push_fence_ring` is retired and dropped from `sunshine.conf` with a report). It
requires the driver capability `CAP_FENCE_RING`; a driver without it, such as an update
that runs only after Windows restarts, makes the host refuse the stream and the console
says to restart Windows and, if that does not help, to run Rubylight setup again to repair
the display driver. The host's compute conversion reads the
textures in place (`idd_push_zero_copy = true`, the default) or its D3D11 device copies
them (`false`).

The driver continues acquiring and copying frames with **D3D11**. The host creates
D3D12-owned shared textures and two shared fences, then submits them with
`IOCTL_SET_FENCE_RING` (`0xB0C`). Atomic slot ownership prevents a producer overwrite while
any host reader holds the image. A written fence orders conversion after the driver's
copy; a release fence covers the last compute and graphics readers before the driver can
reuse a returned slot. The driver picks another free slot or drops a frame instead of
waiting for the host. The ring is replaced before its frame sequence could wrap.

The transport does not use IddCx's native D3D12 swap-chain API and does not add an IddCx
1.11 requirement. The driver still answers the older keyed IOCTL, but the host no longer
sends it. Generation-scoped ring cleanup uses `IOCTL_CLEAR_FRAME_RING_GENERATION`
(`0xB0B`).

If fence setup, shared-resource opening or the first frame fails, the host reports the
failure and ends the stream; it does not fall back to another transport. A failed GPU
ordering operation retires the ring without releasing a potentially busy texture. Device
loss follows the normal capture recovery path.

The two-device native tests in `rust/windows/src/capture/idd_push/fence_tests.rs` cover
SDR/HDR pixels, multiple readers, ring replacement and delayed GPU completion without
installing a driver. Its ignored ABBA transport probe measures producer copy through
completed conversion. These component checks do not establish latency or picture quality
for a live IddCx stream, encoder, network or receiver; the installed-driver evidence comes
from the 3.0 alpha.1 hardware gate.

## Build

Prerequisites (all installed on the development PC on 2026-10-15):

- Visual Studio 2022 Build Tools with `Microsoft.VisualStudio.Component.VC.Tools.x86.x64`
- Windows Driver Kit 10.0.26100 matching the installed SDK
  (`winget install Microsoft.WindowsWDK.10.0.26100`)
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

The driver's GPU-independent retained-frame policy tests also run in the main workspace's
ordinary test suite. To run just these regressions from the repository root (no WDK or GPU):

```powershell
cargo test -p rubylight-vdisplay-proto frame_retry --locked
```

### Probe-only builds (`RLVD_IDDCX_LIB`)

The build links the newest `IddCxStub.lib` in the installed kit (IddCx 1.10, whose table ends
at slot 35). Setting `RLVD_IDDCX_LIB` to a folder with a newer stub (IddCx 1.11 ships in the
WDK 28000 NuGet package) links that one instead and enables the `rlvd_iddcx_newer_stub` cfg,
so `DriverEntry` can call `IddCxCheckOsFeatureSupport` and log whether the OS reports D3D12
swap chains. That build is **probe-only**: its client config is the 1.10 one, which
`IddCxDeviceInitConfig` rejects, so no adapter is created. Cargo prints a warning when the
variable is set, and `rustuild.ps1 -Package` refuses to run with it. Never package or
install such a driver for streaming. Without the variable the probe is skipped (the 1.10
stub has no slot for that DDI) and the log says so.

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
the `Root\RubylightVDisplay` devnode and binds the driver. The host needs no setting: every
stream captures through the frame push, and a failed push ends the stream with a console
warning.

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

- Setup and an in-app update run `install-local.ps1 -InstallerBestEffort` as SYSTEM. 3.0
  needs the driver, so `--no-display-driver` is refused, a quiet update of a 2.6.x install
  that declined it keeps 2.6.x, and an interactive install installs it and clears that
  record. A failure becomes a setup note, like the gamepad driver's.
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
that as its note and the console as the reason there is no virtual display; streams are refused. A fresh install that fails, or whose device does not start, is rolled back
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
