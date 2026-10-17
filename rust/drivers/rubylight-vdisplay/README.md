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

The standard transport uses keyed mutexes and remains the default:

1. The host creates the display with `IOCTL_ADD` and
   keeps it alive with `IOCTL_HEARTBEAT` every second. A monitor belongs to the process that
   created it and departs when that process closes its handles or goes silent for 10 s.
2. The capture backend `idd-push` creates four `SHARED_NTHANDLE | SHARED_KEYEDMUTEX` textures
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

With four textures the host holds at most two (the newest, and the one being encoded), so the
driver always finds one free; `idd_push=debug` logs its counters every 5 s (acquired, pushed,
dropped, overwritten) and `idd_push=trace` each frame's driver time (acquire to publish) and
handoff (publish to the host taking it).

The header also carries the driver's status (size or format mismatch, open failure, wrong
GPU, device lost), the surface it saw and a heartbeat of its swap-chain loop.

### Optional GPU fence transport

The advanced Video setting **Experimental GPU fence transport** (`idd_push_fence_ring =
true`) selects an additional transport for this driver's displays with `capture = auto`
or `idd-push`. It defaults to `false`. End every stream before changing it, then start a
new stream. It requires the driver capability `CAP_FENCE_RING`, compatible AMD compute
capture (`gpu_compute_conversion = true`) and in-place reads (`idd_push_zero_copy = true`).
An older display driver or an incompatible capture device keeps the standard transport
and produces a console warning.

The driver continues acquiring and copying frames with **D3D11**. The host creates
D3D12-owned shared textures and two shared fences, then submits them with
`IOCTL_SET_FENCE_RING` (`0xB0C`). Atomic slot ownership prevents a producer overwrite while
any host reader holds the image. A written fence orders conversion after the driver's
copy; a release fence covers the last compute and graphics readers before the driver can
reuse a returned slot. The driver picks another free slot or drops a frame instead of
waiting for the host. The ring is replaced before its frame sequence could wrap.

This option does not use IddCx's native D3D12 swap-chain API and does not add an IddCx 1.11
requirement. WGC, Desktop Duplication, every encoder choice and the existing keyed IOCTL
remain available. Driver and host capability negotiation allow an old host to use the new
driver's keyed path, and a new host to use an old driver's keyed path. Generation-scoped
ring cleanup uses `IOCTL_CLEAR_FRAME_RING_GENERATION` (`0xB0B`) when supported.

If fence setup, shared-resource opening or the first frame fails, the host retries the
standard keyed ring. A failed GPU ordering operation also retires the fence ring without
releasing a potentially busy texture. Device loss follows the normal capture recovery
path, and the existing WGC fallback still applies if driver capture cannot recover. A
console warning identifies fallback; enabling the setting alone does not prove the
fence path is active.

The two-device native tests in `rust/windows/src/capture/idd_push/fence_tests.rs` cover
SDR/HDR pixels, multiple readers, ring replacement and delayed GPU completion without
installing a driver. Its ignored ABBA transport probe measures producer copy through
completed conversion. These component checks do not establish latency or picture quality
for a live IddCx stream, encoder, network or receiver. Keep this option experimental until
those measurements justify a wider default.

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
the `Root\RubylightVDisplay` devnode and binds the driver. The host needs no setting:
`capture = auto` (the default) uses the frame push for the displays it creates, and an
explicit `capture = wgc` or `ddx` keeps those. A failed frame push falls back to WGC with a
console warning.

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
