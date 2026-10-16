# Third-party code

Rubylight is GPL-3.0-only. Its administration console is a Svelte application served by the Rust host; the bundle includes the Svelte runtime (MIT License, Svelte Contributors), and the console's npm dependencies and their versions are recorded in `rust/web/package-lock.json`. Rust package names, versions, license declarations and repositories are recorded in the packaged `rust-dependencies.json`; exact versions and registry checksums are in `Cargo.lock`. Source is available in the corresponding repository checkout.

External SDKs are not a port of the original host:

- AMF headers 1.5.2 (MIT, Copyright Advanced Micro Devices, Inc.) are vendored unchanged apart from line endings in `rust/windows/include/AMF`, with their license headers; see its README. The AMF runtime is loaded from the installed AMD graphics driver. SDK: https://github.com/GPUOpen-LibrariesAndSDKs/AMF.
- PyroWave `502a3b52a39312ab82c85b1e2fc0e746faee91a4`, API 1.0.0 (bitstream v1, frozen 2026-10-03 and identical on the wire to the `186f0393` bitstream Vibepollo 2.0 and its clients use); Granite `fb178c8080d163419e8d20f10715c61c53c1ec9b`. `rust/tools/build_pyrowave.sh` builds the pinned upstream SDK and applies the patches in `rust/codec-patches/pyrowave`: Vibepollo 2.0's 4:4:4 payload sizing (0002) and short-decoder-block check (0003), from Punktfunk's patches 0001 and 0008 (see [PyroWave patches from Punktfunk](#pyrowave-patches-from-punktfunk)). The encoder buffer-pool patch (Vibepollo's 0001, Punktfunk's 0004) is no longer needed: upstream now keeps those buffers in each frame context. Punktfunk's global-priority queue patch (0005) is not needed either: `pyrowave_create_device_by_compat` takes the priority, which the host sets to HIGH or REALTIME. Their source patches are retained; packaged `pyrowave-build-info.txt` records revisions and patches. MIT notices are included for PyroWave/Granite/volk. Source: https://github.com/Themaister/pyrowave/tree/502a3b52a39312ab82c85b1e2fc0e746faee91a4.
- The Opus DLL comes from MSYS2 UCRT64. Its installed license notice, and those of the GCC and winpthreads runtimes, are copied into the package. MSYS2 source/build recipes: https://github.com/msys2/MINGW-packages.
- Virtual gamepad driver libvirtualgamepad v0.1.0-beta.6 (MIT, https://github.com/Nonary/libvirtualgamepad), with its catalog, tools and install scripts as released in Vibepollo 2.0.0 (GPL-3.0, https://github.com/Nonary/Vibepollo) and signed by the SignPath Foundation, plus the Sunshine Playnite Connector plugin from the same release. `rust/build.ps1` extracts them from the pinned Vibepollo 2.0.0 installer (SHA-256 `7b3500ec0c774644ce5a435a48f61c046c48494d0f18b67afa0b3561931794b7`), checks the driver signatures, and records them with `nefconc.exe` (https://github.com/nefarius/nefcon), which Rubylight's own display driver uses to create its device, in the packaged `drivers.txt`. Rubylight no longer ships the libvirtualdisplay virtual display driver; setup removes it from earlier installations. No ViGEmBus driver or client code is included.
- usbip-win2 0.9.8.1 (BSD-2-Clause, Copyright 2021-2026 Vadym Hrynchyshyn), https://github.com/vadimgrn/usbip-win2/tree/v.0.9.8.1. The original `USBip-0.9.8.1-x64.exe` is bundled unchanged with SHA-256 `38cad6d4432b52d5bb9409d9ad03b72fdffc4ada4cd3a48fbeca1a2752a8518a`. Its Authenticode signature is verified before packaging and installation. It contains the upstream Microsoft-signed drivers and original bundled runtime/license materials. The full BSD notice is shipped in `drivers/usbip/LICENSE.txt` and `licenses/usbip-win2.txt`. Rubylight's optional setup wrapper was added on 2026-10-13; the third-party installer is not modified or re-signed.
- Windows GPU, audio, input, service and security APIs are imported through Microsoft's Rust `windows` crate. Vulkan headers define the PyroWave ABI; Vulkan is supplied by the installed graphics driver.
- The host compiles its embedded HLSL GPU conversion shaders through Windows' D3DCompiler API. These shaders are part of Rubylight's source; the Windows compiler and GPU driver remain system dependencies.

The standalone C programs under `rust/tests` are independent interoperability fixtures. Moonlight-common-c, nanors, FFmpeg decoding, OpenSSL and Opus are used by these fixtures and are not compiled as original host implementation code.

## Composite DualSense haptics

The USB/IP audio and haptics implementation was adapted to Rust on 2026-10-13 from [Nonary/Vibepollo 8b3ea1c4](https://github.com/Nonary/Vibepollo/tree/8b3ea1c4a41c383752475ffa551168a16b024d48), under GPL-3.0-or-later (used here under GPLv3). USB descriptors, reports and calibration derive from [Nonary/libvirtualgamepad 9edbce11](https://github.com/Nonary/libvirtualgamepad/tree/9edbce11ad71e9455867c356c8368a6b39fa2242), by Chase Payne, and HIDMaestro’s composite profile under the MIT terms below. Windows packages bundle the optional usbip-win2 installer described above, shared by native Steam Deck controllers and composite DualSense haptics.

MIT License

Copyright (c) 2026 Chase Payne
Copyright (c) 2026 HIDMaestro Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## PyroWave patches from Punktfunk

The PyroWave SDK patches `0002-payload-data-444-sizing` and `0003-decoder-reject-short-block` in `rust/codec-patches/pyrowave` are adapted from patches 0001 and 0008 of [Punktfunk](https://git.unom.io/unom/punktfunk) (MIT OR Apache-2.0), used here under the MIT License. They are compiled into the shipped `libpyrowave-shared-1.dll`.

## Rubylight virtual display driver

`rust/drivers/rubylight-vdisplay` (the UMDF IddCx driver), `rust/drivers/rubylight-vdisplay-proto` (its host-driver protocol, used by the host) and the host's `idd-push` capture are based on punktfunk's `pf-vdisplay`, `wdk-iddcx`, `pf-umdf-util` and `pf-driver-proto` by Enrico Bühler (unom), [punktfunk 0.44.0, commit c5ed113](https://github.com/unom/punktfunk), forked on 2026-10-15 under MIT OR Apache-2.0 and kept under those licenses; the full texts are in `rust/drivers/rubylight-vdisplay/LICENSE-MIT` and `LICENSE-APACHE`. Rubylight changed the driver's identity, control protocol and video transport (a host-shared texture ring instead of in-driver encoding) and removed its encoder, hardware cursor and remote-session adapter. punktfunk's INF was itself adapted from [MolotovCherry/virtual-display-rs](https://github.com/MolotovCherry/virtual-display-rs) (MIT) and SudoVDA's control-device DACL. The driver workspace vendors [microsoft/windows-drivers-rs](https://github.com/microsoft/windows-drivers-rs) 0.5.1 (`wdk-build`, `wdk-sys`; Copyright (c) Microsoft Corporation, MIT OR Apache-2.0) with punktfunk's added IddCx bindings. The driver is not part of the installer yet.

MIT License

Copyright (c) 2026 unom - Enrico Bühler

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
