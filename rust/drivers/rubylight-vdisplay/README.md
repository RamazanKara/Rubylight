# Rubylight virtual display driver

`rubylight-vdisplay` is a UMDF IddCx virtual display driver for Rubylight, based on
[punktfunk](https://github.com/unom/punktfunk)'s `pf-vdisplay` by Enrico Bühler (unom),
MIT OR Apache-2.0 (see `LICENSE-MIT`, `LICENSE-APACHE`). It creates virtual monitors on the
Rubylight host's request and pushes every frame Windows composes for them to the host
through shared GPU textures, so the host converts and encodes them without WGC or Desktop
Duplication.

This is phase 2 groundwork. It is off by default, is not part of the installer, and has not
been installed or run on hardware yet.

| | |
|---|---|
| Hardware ID | `Root\RubylightVDisplay` |
| UMDF service | `RubylightVDisplay` |
| Files | `rubylight_vdisplay.inf`, `rubylight_vdisplay.dll`, `rubylight_vdisplay.cat` |
| Control interface | `{5e240e6b-b196-4295-81c3-533c10000c59}`, IOCTL functions `0xB00`-`0xB0A` |
| Reserved for in-driver encode | IOCTL functions `0xB10`-`0xB1F` |

None of these collide with libvirtualdisplay (Vibepollo) or punktfunk, so all three can be
installed side by side.

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

1. The host (`virtual_display_driver = rubylight`) creates the display with `IOCTL_ADD` and
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
4. The host takes the newest published slot with key 1, copies it into its own pool (on the
   compute queue on AMD), releases with key 0 and stamps the image with the present QPC.

The header also carries the driver's status (size or format mismatch, open failure, wrong
GPU, device lost), the surface it saw and a heartbeat of its swap-chain loop.

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
the `Root\RubylightVDisplay` devnode and binds the driver. Then, in the host's config:

```
virtual_display_driver = rubylight
capture = idd-push
```

`capture = auto` also uses the frame push for displays created by this driver; an explicit
`wgc` or `ddx` keeps those. A failed frame push falls back to WGC with a console warning.

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

and remove `virtual_display_driver` and `capture = idd-push` from the config. The driver keeps
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

The setup program does not call it yet. To ship: put the unsigned package and
`install-local.ps1` under `driversubylight-display` in the package, run it from setup when
the user turns on the Rubylight display, and run `-Uninstall` from the uninstaller. Once
SignPath signs the catalog, setup installs that package instead and keeps only the
publisher certificate in `TrustedPublisher` (below); `-Uninstall` cleans up a locally
signed install either way.

## Release signing

A release package must be signed in this order, because the catalog hashes the signed DLL:
sign `rubylight_vdisplay.dll`, run Inf2Cat, sign `rubylight_vdisplay.cat`. With SignPath that
is two signing requests (or one per file type in a single artifact configuration that signs
the DLL before the catalog is generated). A package signed with a publicly trusted, non
Microsoft certificate still prompts unless its certificate is in Trusted Publishers; a silent
install on users' PCs needs Microsoft attestation signing (an EV certificate and Partner
Center) or the installer adding the publisher certificate to Trusted Publishers.
