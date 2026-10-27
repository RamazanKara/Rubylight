# Third-party code

Rubylight is GPL-3.0-only. Its administration console is a Svelte application served by the Rust host; the bundle includes the Svelte runtime (MIT License, Svelte Contributors), and the console's npm dependencies and their versions are recorded in `rust/web/package-lock.json`. Rust package names, versions, license declarations and repositories are recorded in the packaged `rust-dependencies.json`; exact versions and registry checksums are in `Cargo.lock`. The exception is rusty_enet 0.4.0 (ENet 1.3.18, MIT), vendored with changes in `rust/vendor/rusty_enet` (listed in its `Cargo.toml`, notice in `NOTICE`), which has no registry checksum there. Source is available in the corresponding repository checkout.

The negotiated HEVC intra-refresh policy follows Eftimin70's Xbox workaround and the verified finite-GOP/CTB-row configuration in [Nonary/vibeshine commit 5671498](https://github.com/Nonary/vibeshine/commit/5671498664f5a8f14e959d4ba05904f11c92525c). Rubylight's Rust implementation adds paired-setting verification and restores its previous policy when the driver rejects the workaround. Startup and recovery keyframe requests remain enabled. This adaptation remains under GPL-3.0-only.

The statistical video FEC (`rust/core/src/statistical_fec.rs`) is ported from [AlkaidLab/foundation-sunshine](https://github.com/AlkaidLab/foundation-sunshine) commit 880f1ef, `src/streaming/fec.cpp` and `docs/dynamic-fec.zh-CN.md`; the encoder/FEC budget split in `Session::encoder_bitrate` follows its `src/video.cpp`. Both are GPL-3.0 and remain so here.

External SDKs are not a port of the original host:

- AMF headers 1.5.2 (MIT, Copyright Advanced Micro Devices, Inc.) are vendored unchanged apart from line endings in `rust/windows/include/AMF`, with their license headers; see its README. The AMF runtime is loaded from the installed AMD graphics driver. SDK: https://github.com/GPUOpen-LibrariesAndSDKs/AMF.
- PyroWave `502a3b52a39312ab82c85b1e2fc0e746faee91a4`, API 1.0.0 (bitstream v1, frozen 2026-10-03 and identical on the wire to the `186f0393` bitstream Vibepollo 2.0 and its clients use); Granite `fb178c8080d163419e8d20f10715c61c53c1ec9b`. `rust/tools/build_pyrowave.sh` builds the pinned upstream SDK and applies the patches in `rust/codec-patches/pyrowave`: Vibepollo 2.0's 4:4:4 payload sizing (0002) and short-decoder-block check (0003), from Punktfunk's patches 0001 and 0008 (see [PyroWave patches from Punktfunk](#pyrowave-patches-from-punktfunk)). The encoder buffer-pool patch (Vibepollo's 0001, Punktfunk's 0004) is no longer needed: upstream now keeps those buffers in each frame context. Punktfunk's global-priority queue patch (0005) is not needed either: `pyrowave_create_device_by_compat` takes the priority, which the host sets to HIGH or REALTIME. Their source patches are retained; packaged `pyrowave-build-info.txt` records revisions and patches. MIT notices are included for PyroWave/Granite/volk. Source: https://github.com/Themaister/pyrowave/tree/502a3b52a39312ab82c85b1e2fc0e746faee91a4.
- The Opus DLL comes from MSYS2 UCRT64. Its installed license notice, and those of the GCC and winpthreads runtimes, are copied into the package. MSYS2 source/build recipes: https://github.com/msys2/MINGW-packages.
- FFmpeg 8.1.2 (LGPL 2.1 or later) for audio passthrough: `rust/tools/build_ffmpeg.sh` builds `avcodec-62.dll` and `avutil-60.dll` from the official source release (SHA-256 `464beb5e7bf0c311e68b45ae2f04e9cc2af88851abb4082231742a74d97b524c`) with only the AC3 and E-AC3 encoders, decoders and parser, never `--enable-gpl` or `--enable-nonfree`, and checks that they import only Windows DLLs and each other (avutil links MinGW-w64's winpthreads statically; its notice is copied into the package with the runtimes' above). The host loads them at run time (`rust/windows/src/ffmpeg.rs`) and offers only Opus and PCM without them, so a user can swap in their own build of the same version. `COPYING.LGPLv2.1`, FFmpeg's `LICENSE.md` and `build-info.txt` (source checksum, configure line, compiler) are copied into the package under `licenses/ffmpeg`. Source: https://ffmpeg.org/releases/ffmpeg-8.1.2.tar.xz.
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

## SADIE II HRTF data

Headphone surround renders with head-related impulse responses derived from the SADIE II database (subject D1, the Neumann KU100 dummy head, 48 kHz), Copyright 2018, University of York, AudioLab, Department of Electronic Engineering, licensed under the Apache License, Version 2.0: [Zenodo record 10886409](https://zenodo.org/records/10886409), file `D1_HRIR_SOFA.zip`. Rubylight changed them: it picked nine horizontal directions, applied diffuse-field equalization, made each filter minimum phase with its measured arrival time, symmetrized the left and right halves, trimmed the common onset and shortened them to 256 taps. `rust/tools/hrir/generate.py` makes `rust/core/src/audio_dsp/hrir_sadie2_ku100_48k.bin` from the original download and records each step in its `.json` manifest; the upstream license is also in `rust/core/src/audio_dsp/LICENSE-SADIE-II.txt`.

Citation: C. Armstrong, L. Thresh, D. Murphy and G. Kearney, "A Perceptual Evaluation of Individual and Non-Individual HRTFs: A Case Study of the SADIE II Database", Applied Sciences 8(11):2029, 2018, https://doi.org/10.3390/app8112029.

```text

                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "[]"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright [yyyy] [name of copyright owner]

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
```
