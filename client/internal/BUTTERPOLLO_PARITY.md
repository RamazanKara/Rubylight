# Android client parity log (historical)

> Historical record of how the Android client caught up with the Rubylight host, written while the
> app was called Butterpollo and then Rubylight Android; it is Nightjar now. Rubylight below means
> the host. See [RENAME.md](../docs/RENAME.md).
>
> Current status: [PARITY-MATRIX.md](../docs/PARITY-MATRIX.md) (against the host) and
> [CLIENT-PARITY.md](../docs/CLIENT-PARITY.md) (Android against desktop). Current real-device
> checks: [device-checklist.md](device-checklist.md). The dated milestone notes this file used to
> carry (milestones 4, 5 and 7, the 2026-10-08 and 2026-10-09 audits, client-side upscaling) are in
> its git history (`git log -p -- client/internal/BUTTERPOLLO_PARITY.md`). This file keeps a short
> summary, the credits, and the PyroWave decoder record, whose rebuild recipe is still in use.

## Done

Implemented between 2026-10-07 and 2026-10-09 and carried into Nightjar:

- Latency statistics: input-to-output, decode and host processing per frame, in the overlay and a
  CSV export; render and scanout times read as unavailable where Android does not report them.
- PyroWave on Android: the Vulkan decoder (64-bit, Vulkan 1.1 or newer) on the stream's
  SurfaceView, 8- and 10-bit, 4:2:0 and 4:4:4, HDR10 with mastering metadata, record framing that
  conceals lost records, its own bitrate controller and a bandwidth test (milestone 4b below).
- Per-PC streaming settings keyed by host UUID; fractional refresh rates to 0.01 Hz and render
  scale sent to Rubylight; native resolutions; virtual display requests where advertised.
- HDR10 with host metadata (control type `0x010e`), 10-bit SDR, and YUV 4:4:4 where the decoder
  has the profile.
- The local stream menu (overlay, touch mode, keyboard, on-screen controls, reconnect,
  disconnect), PiP with Disconnect, and settings search.
- Automatic bitrate on Rubylight hosts from loss, connection status and decode time, with
  PyroWave's own controller.
- One-time PIN pairing, device permissions, app UUIDs and versioned artwork, and backups that
  leave out the client identity so a restored install pairs again.
- Remote Monitor and Input-only roles with scoped disconnect and confirmations for replacing or
  ending another device's session.
- Controller button and axis mapping per controller model, USB DualSense and DualSense Edge
  through Nightjar's USB driver (off by default), per-game keyboard and mouse buttons.
- Text clipboard and server commands over paired HTTPS, pause status and the frame limiter.
- Multi-interface Wake-on-LAN, Android 15 frame-rate cadence hints and Android 16 adaptive
  refresh, adaptive layouts for foldables and large screens.
- Client-side upscaling for MediaCodec SDR: Bilinear, FSR 1.0 and SGSR 1 on a GLES 3.0 thread.

## Open

- Real-device checks: [device-checklist.md](device-checklist.md) is the list; the PyroWave ones
  are under [Exact real-device checks still required](#exact-real-device-checks-still-required).
- Removal of the classic C engine ([RUST_CORE.md](RUST_CORE.md)).
- Recorded on 2026-10-09 as protocol boundaries: virtual-display layouts, desktop scale and
  retained displays are set on the host, not by launch options; host administrator settings need
  the host's own authentication, which a pairing certificate does not grant; the video header
  carries one host processing time, so the overlay never derives encode-only or end-to-end time
  from it.

## Credits

Artemis Android was inspected at
[`c5cf27f4`](https://github.com/ClassicOldSong/moonlight-android/tree/c5cf27f4dc822db0e863c4691e7a70c74bea977a),
with common-c [`c9994368`](https://github.com/ClassicOldSong/moonlight-common-c/blob/c999436858471dfefa7617af3b7dc03ec1644ce4/src/ControlStream.c).
Its GPL-3.0 license permits reuse. The extras of milestone 5 (clipboard, server commands, pause
status, frame limiter) follow its protocol approach, retain Moonlight attribution, and leave the
pinned common-c submodule unchanged.

PyroWave and Granite are by Hans-Kristian Arntzen under the
[PyroWave MIT license](https://github.com/Themaister/pyrowave/blob/502a3b52a39312ab82c85b1e2fc0e746faee91a4/LICENSE)
and the [Granite MIT license](https://github.com/Themaister/Granite/blob/fb178c8080d163419e8d20f10715c61c53c1ec9b/LICENSE);
their notices and volk's are packaged in `assets/pyrowave_notices.txt`. The Android renderer port
is credited in [milestone 4b](#milestone-4b-vulkan-decoder-port-historical-implementation-record).

The [AMD source](https://github.com/GPUOpen-Effects/FidelityFX-FSR/tree/a21ffb8f6c13233ba336352bdff293894c706575)
is adapted to GLES FP32 with explicit OES taps instead of gather4, native high-precision
arithmetic and finite flat-color divisions. The complete MIT notice is packaged in
[NOTICE-FSR1.txt](../app/src/main/assets/NOTICE-FSR1.txt).

SGSR 1 uses [Qualcomm's mobile RGBA shader](https://github.com/SnapdragonGameStudios/snapdragon-gsr/blob/d926f074bcb9d714e179f1ce0fcb9ee2eeb5074e/sgsr/v1/include/glsl/sgsr1_shader_mobile.frag),
with its 12-tap Lanczos-like filter, green-channel edge detection, 8/255 edge threshold and
23/255 correction limit. Explicit crop-clamped OES taps replace texture gathers; FP32 arithmetic
keeps divisions finite. The complete BSD-3-Clause notice, both upstream copyright years, revision
and adaptations are packaged in [NOTICE-SGSR1.txt](../app/src/main/assets/NOTICE-SGSR1.txt).

## Milestone 4b: Vulkan decoder port (historical implementation record)

The implementation notes and measurements in this section describe that milestone. The current
feature status and remaining work are in [PARITY-MATRIX.md](../docs/PARITY-MATRIX.md) and
[device-checklist.md](device-checklist.md).

The second attempt ports the Android renderer from
[joemossjr16/artemis-android-pyrowave `387d3a5c`](https://github.com/joemossjr16/artemis-android-pyrowave/tree/387d3a5ce1e3df8d4a4d29eec5b81a7b904926a5).
Its [LICENSE.txt](https://github.com/joemossjr16/artemis-android-pyrowave/blob/387d3a5ce1e3df8d4a4d29eec5b81a7b904926a5/LICENSE.txt)
is GPL-3.0, compatible with this GPL-3.0 Moonlight-derived client. Attribution remains in source,
the application license, and packaged `assets/pyrowave_notices.txt`. PyroWave, Granite and volk's
full MIT notices are included there. The necessary new runtime dependency is `libpyrowave-shared.so`.
Both ARM64 and x86-64 libraries were rebuilt with Windows NDK 29, rather than copied from the fork.
32-bit APKs retain the ordinary decoders and tolerate the absent Vulkan library.

Pinned dependency sources: PyroWave `7fe0e2667cd3076b8b2bf04bcb80bc8d691b800b`, Granite
`53090429539bf3404250d18fe58db971f0907c04`, volk `47cddf7ed97b94118a08aacb548a411188e016cc`,
Vulkan-Headers `6802bb4733b63ed5efd3adb308a6c885ef180ea1`. The decoder short-block patch from
Rubylight `6772d401` is retained unchanged in `jni/pyrowave-renderer/patches/`. Two additional
patches correct metadata read synchronization and the initial transition of aliased linear
payload images; see the rebuild notes below. Sources/headers are MIT; the small Android
renderer and common-c integration remain GPL-3.0.

### Negotiation and output

- **Prefer PyroWave** explicitly opts into the experimental decoder and uses **SurfaceView**.
  Existing HDR and 4:4:4 preferences control its profile bits; Auto keeps conventional codecs.
  The quality pass changed this from automatic selection because normal Moonlight bitrates are
  too low for PyroWave. Ordinary codec fallback remains available; start near 280 Mbps for 720p/60
  on a fast LAN and verify quality and bandwidth on the actual host/device.
- Android API 29+, a 64-bit library and Vulkan **1.1** are necessary. Runtime checks include
  subgroup basic/vote/ballot/arithmetic/shuffle/shuffle-relative operations, compute stage and size
  control (4–128), full subgroups, shaderInt16, storageBuffer16BitAccess and
  shaderStorageImageWriteWithoutFormat. On Vulkan 1.3 they also require storageBuffer8BitAccess,
  timelineSemaphore and synchronization2. Below 1.3 the features come from extensions
  (`jni/pyrowave-renderer/device_path.h`): 8-bit storage may be missing when
  `maxTexelBufferElements` is at least 16M, decode and presentation share one queue, and decode
  completion is a fence. `adb shell setprop debug.nightjar.pyro_vk11 1` puts a newer device on that
  path for testing; the next stream's log says so. Float16 arithmetic
  is optional. Checks also cover graphics+compute+present queue support, scratch image formats,
  dimensions, layers and workgroup limits. These are conservative requirements of this port.
- After server capability intersection, the client creates the decoder, swapchain and colour pipeline
  on the **actual stream Surface**, then executes a zero-coefficient grey frame into the decode planes.
  This warm-up is not presented or included in stream statistics. A failed profile tries the next
  compatible profile; failure of all PyroWave profiles leaves conventional codec negotiation intact.
  The prepared renderer is reused by video setup and released when RTSP selects a conventional codec.
- Native RTSP requires `PYROWAVE/90000` and the exact `x-ss-pyrowave.bitstream:186f0393` attribute.
  Missing/different IDs select AV1/HEVC/H.264. SDP sends `bitStreamFormat=3`, the selected chroma and
  HDR flags. It deliberately omits **both** `pyrowaveAdaptiveFec` and `pyrowaveFeatures`; even a zero
  adaptive-FEC attribute selects a different framing protocol on Rubylight.
- The ordinary core FEC/depacketizer supplies complete opaque frames with its eight-byte short
  header and trailing FEC padding removed. The renderer accepts little-endian packet count/lengths,
  validates exact boundaries and block sizes, then feeds the packets to the pinned decoder. It does
  not accept the fork's older `PYRW` container or partially recovered record-mode frames. Corrupt
  frames are discarded, and the next complete intra frame recovers. The core submodule remains pinned;
  `android_rtsp.c` and `android_sdp.c` are attributed copies of its two modified translation units.
- Eight-bit output uses R8_UNORM planes; ten-bit output uses **R16_UNORM** throughout the decoded
  planes, including full-size chroma for 4:4:4. The shader matches Rubylight's code/1023 encoding,
  64–940 luma, midpoint 512/scale 896 chroma, optional full range, centred 4:2:0, and BT.709 SDR or
  BT.2020 non-constant-luminance HDR. HDR RGB remains PQ encoded, with no sRGB framebuffer conversion.
- HDR requires an A2B10G10R10 or A2R10G10B10 UNORM + `HDR10_ST2084_EXT` surface pair,
  `VK_EXT_swapchain_colorspace`, and `VK_EXT_hdr_metadata`. The same ten-bit format must also support
  SDR presentation, preserving ten-bit SDR when the host turns HDR off. Mode changes recreate the
  swapchain. The Android 13 and 14 loaders never list `HDR10_ST2084_EXT`; there a ten-bit
  `SRGB_NONLINEAR` swapchain is used and HDR is signalled with `ANativeWindow_setBuffersDataSpace`
  (`ADATASPACE_BT2020_PQ`, `ADATASPACE_SRGB` when off), re-applied after every swapchain creation
  because the loader resets the dataspace there. The shader's PQ output keys off the HDR flag. Mastering primaries/white point, luminance and MaxCLL/MaxFALL are passed with Vulkan units;
  absent/short metadata clears old values to unknown. The unsupported trailing full-frame field is
  omitted. [Vulkan metadata units](https://docs.vulkan.org/refpages/latest/refpages/source/VkHdrMetadataEXT.html)
  and [colour-space definitions](https://docs.vulkan.org/refpages/latest/refpages/source/VkColorSpaceKHR.html)
  govern this path; MediaCodec HDR keys are not used for the Vulkan surface.
- Decode has its own fence, including when swapchain acquisition times out or requires recreation.
  PyroWave disables direct submission so GPU/display waits run on the core's decoder thread.
  Its CLOCK_MONOTONIC completion feeds milestone 1 `FrameLatencyStats` and the existing CSV/overlay.
  Input-to-output includes parsing/upload/queue wait and GPU completion, but excludes subsequent
  image acquisition/present. Separate GPU timestamps bracket the GPU decode. Render/scanout time
  stays unavailable; no fabricated render callback or rendered-FPS count. Fatal runtime decoder
  errors end the connection; they do not attempt an in-place codec switch. Reconnect with explicit
  HEVC/AV1 for a driver failure occurring after successful preparation.

### Measured synthetic frames

On 2026-10-07, `app/src/test/native/pyrowave_benchmark.cpp` used the pinned upstream Windows encoder
to generate 1280×720 luma ramps/chroma checkerboards, packetized at 1024 bytes into the same container
the Android renderer accepts. The two frames were 501,996 (4:2:0) and 502,656 (4:4:4) bytes, with a
500,000-byte codec budget. Each of four runs discarded 10 warm-up decodes and measured 60 completed
decodes; submissions were spaced by at least 17 ms and the process was restricted to two CPUs.

The GPU selected by the upstream C API was **AMD Radeon RX 7900 XT**, Vulkan 1.4.349, raw driver
version 8389003. This was the existing Windows toolchain build of the same pinned C API; DLL SHA-256
`090423bf0054353f29b3cf6ac7cfab76a59cdc7bec369ae8c0564fdb825f8a31`.
`PyroWaveBenchmark.java` feeds the native monotonic input/completed-output timestamps into the actual
milestone 1 statistics class. The recorded input is checked in at
`app/src/test/resources/pyrowave/windows-720p.csv`; the adapter can regenerate all four milestone 1 CSVs.

| 720p, 8-bit | Completed frames | Input → output avg | p95 | p99 |
| --- | ---: | ---: | ---: | ---: |
| 4:2:0 compute | 60 | 15.850740 ms | 44.936700 ms | 110.844200 ms |
| 4:2:0 fragment | 60 | 34.978430 ms | 135.435400 ms | 203.041700 ms |
| 4:4:4 compute | 60 | 26.796847 ms | 93.904900 ms | 126.660700 ms |
| 4:4:4 fragment | 60 | 24.810995 ms | 75.169600 ms | 108.253700 ms |

These are **Windows synchronous GPU decode + CPU readback** timings on a shared, CPU-constrained
machine, with large scheduling/contention tails. They are neither GPU-only nor Android renderer
measurements, and establish no mobile performance target. Receive/render timestamps are left blank.
The zero-coefficient warm-up was separately checked through the upstream compute and fragment paths
for both chroma modes, producing grey pixels. No Android device/emulator was connected; Android
surface output, ten-bit visual accuracy, HDR transitions, live transport and mobile timing remain
unverified. The shaders compile and pass SPIR-V validation; that does not certify display output.

To repeat the Windows benchmark from this repository in PowerShell (MSYS2 UCRT64 compiler, Windows
executables; use a matching C API installation in `$PyroInstall`):

```powershell
$Bench = Join-Path $env:TEMP 'bp-pyrowave-bench'
New-Item -ItemType Directory -Force $Bench | Out-Null
$PyroInstall = 'C:/Users/ramaz/git/pyrowave-build/install-502a3b52'
$VulkanHeaders = 'C:/Users/ramaz/git/pyrowave-build/src-502a3b52/Granite/third_party/khronos/vulkan-headers/include'
$env:PATH = 'C:/msys64/ucrt64/bin;' + $env:PATH
g++ -std=c++17 -O2 app/src/test/native/pyrowave_frame_test.cpp -o "$Bench/frame-test.exe"
& "$Bench/frame-test.exe"
g++ -std=c++17 -O2 -Iapp/src/main/jni/pyrowave-renderer/prebuilt/include "-I$VulkanHeaders" app/src/test/native/pyrowave_benchmark.cpp "$PyroInstall/lib/libpyrowave-shared.dll.a" -o "$Bench/benchmark.exe"
Copy-Item "$PyroInstall/bin/libpyrowave-shared-1.dll" $Bench
$p = Start-Process "$Bench/benchmark.exe" -WorkingDirectory $Bench -WindowStyle Hidden -PassThru -RedirectStandardOutput "$Bench/measurements.csv" -RedirectStandardError "$Bench/benchmark.log"
$p.ProcessorAffinity = 3
$p.WaitForExit()
if ($p.ExitCode -ne 0) { throw 'Benchmark failed; inspect benchmark.log' }
javac -J-XX:ActiveProcessorCount=2 -d "$Bench/classes" app/src/main/java/com/limelight/binding/video/FrameLatencyStats.java app/src/test/java/com/limelight/binding/video/PyroWaveBenchmark.java
java -XX:ActiveProcessorCount=2 -cp "$Bench/classes" com.limelight.binding.video.PyroWaveBenchmark "$Bench/measurements.csv" $Bench
```

### Rebuilding the bundled decoder

Run from the repository root. Use a fresh task directory on D: when available; normal Gradle
builds consume the committed prebuilts, so no downloads or CMake build run during ordinary
application builds. No owner checkout, system services or drivers are modified by these commands.

Apply all three patches in filename order. `0003` retains the malformed short-block rejection.
`0004` includes storage-buffer reads in the post-upload dependency: dequantization metadata
is always an SSBO, even when payload data uses sampled texel buffers or linear images.
`0005` suppresses Granite's automatic initial image transition. LinearHost still creates the
Vulkan image as PREINITIALIZED, and the existing first upload performs its sole transition
to GENERAL. Subsequent uploads retain GENERAL. Neither patch changes the public API,
bitstream, precision settings or embedded SDK shaders.

```powershell
$Repo = (Get-Location).Path
$Work = if (Test-Path 'D:/') { 'D:/CodexArtifacts/nightjar-pyrowave-rebuild' } else { Join-Path $env:TEMP 'nightjar-pyrowave-rebuild' }
if (Test-Path -LiteralPath $Work) { throw 'Use a fresh rebuild directory.' }
$Source = "$Work/pyrowave"
$Sdk = 'C:/Users/ramaz/AppData/Local/Android/Sdk'
$Ndk = "$Sdk/ndk/29.0.14206865"
$env:PATH = "$Sdk/cmake/3.22.1/bin;" + $env:PATH
git init $Source
git -C $Source remote add origin https://github.com/Themaister/pyrowave.git
git -C $Source fetch --depth 1 origin 7fe0e2667cd3076b8b2bf04bcb80bc8d691b800b
git -C $Source checkout --detach FETCH_HEAD
git init "$Source/Granite"
git -C "$Source/Granite" remote add origin https://github.com/Themaister/Granite.git
git -C "$Source/Granite" fetch --depth 1 origin 53090429539bf3404250d18fe58db971f0907c04
git -C "$Source/Granite" checkout --detach FETCH_HEAD
git -C "$Source/Granite" submodule update --init --jobs 2 third_party/volk third_party/khronos/vulkan-headers
Get-ChildItem "$Repo/app/src/main/jni/pyrowave-renderer/patches/*.patch" | Sort-Object Name | ForEach-Object {
    git -C $Source apply $_.FullName
    if ($LASTEXITCODE -ne 0) { throw "SDK patch failed: $($_.Name)" }
}
foreach ($abi in @('arm64-v8a', 'x86_64')) {
    cmake -S $Source -B "$Work/$abi" -G Ninja "-DCMAKE_TOOLCHAIN_FILE=$Ndk/build/cmake/android.toolchain.cmake" "-DANDROID_ABI=$abi" -DANDROID_PLATFORM=android-29 -DANDROID_STL=c++_static -DCMAKE_BUILD_TYPE=Release -DANDROID_SUPPORT_FLEXIBLE_PAGE_SIZES=ON -DPYROWAVE_FP32_MATH=ON -DPYROWAVE_FP32_STORAGE=OFF -DPYROWAVE_DEVEL=OFF -DPYROWAVE_UTILS=OFF
    if ($LASTEXITCODE -ne 0) { throw "SDK configure failed: $abi" }
    cmake --build "$Work/$abi" --target pyrowave-shared --parallel 2
    if ($LASTEXITCODE -ne 0) { throw "SDK build failed: $abi" }
    Copy-Item "$Work/$abi/libpyrowave-shared.so" "app/src/main/jni/pyrowave-renderer/prebuilt/$abi/"
    & "$Ndk/toolchains/llvm/prebuilt/windows-x86_64/bin/llvm-strip.exe" --strip-unneeded "app/src/main/jni/pyrowave-renderer/prebuilt/$abi/libpyrowave-shared.so"
}
```

The 2026-10-17 combined rebuild used Windows NDK `29.0.14206865` (Clang 21, build
`13989888` / `r563880c`), SDK CMake `3.22.1-g37088a8-dirty` and its bundled Ninja, with the
pins and explicit options above. Both libraries retain all 48 exported API names, the SONAME,
Android system-library dependencies and 16 KiB ELF LOAD alignment. The stripped artifacts are:

| ABI | Bytes | SHA-256 |
| --- | ---: | --- |
| arm64-v8a | 1,233,704 | `6f7626d783ad1aeaa0181f2cb3a0608cd3887b821fa9807cd34e032f76e7ea4e` |
| x86_64 | 1,284,248 | `8a5bbda825985392eb5acc56b2fe7ec955b0433b882f5d508a0f4e3f3da69adc` |

On 2026-10-20 the libraries were rebuilt with the same toolchain, options and three patches at
PyroWave `a0d2e71e`, which changes only the fragment iDWT shader (`shaders/idwt.frag` and its
embedded SPIR-V): the first two outputs at an edge now apply the edge mirroring, and the CbCr
mirroring for multi-plane input is no longer compiled out by a misnamed macro. The fragment path is the one
upstream selects on Qualcomm drivers. Granite, the C API, the bitstream and the patches are
unchanged; all 48 exports, the SONAME and 16 KiB LOAD alignment were rechecked.

| ABI | Bytes | SHA-256 |
| --- | ---: | --- |
| arm64-v8a | 1,234,792 | `4239343a6a15194c2c82ce881767f48fd6beb9e9901bbad6e1ac4c76fd4cf48b` |
| x86_64 | 1,285,336 | `50ef087792580309ce5e3f135e25090d47792e78fffcde0c7c4bfbffab165283` |

These are artifact identities, not a claim of path-independent reproducible binaries. The
ARM64 device comparison on Fold7 with active Turnip Gen8 V37 tested a rebuilt control
(short-block patch only), metadata barrier only, and both synchronization patches. All three
still showed the chart corruption with 0% reported missing records. These fixes correct
conditional synchronization defects; they do not resolve that reported visual corruption.
The combined rebuild rendered the same HDR chart correctly with the system driver and
also completed the native-resolution moving-scene checks on that driver.

On 2026-11-04 the libraries were rebuilt with the same toolchain, options and three patches at
PyroWave `7fe0e266` with its Granite `53090429`. PyroWave `572bfad4` lowers its requirement to
core Vulkan 1.1 with extensions, and that Granite emulates synchronization2 and works without
timeline semaphores. The decoder reads its payload through texel buffers when 8-bit storage is
missing and `maxTexelBufferElements` is at least 16M. The C API is unchanged (1.0.0, the same
48 exports), and the SONAME and 16 KiB LOAD alignment were rechecked; the header changes only in
encoder comments. The three patches apply unchanged but for line offsets in `0004` and `0005`.

Between `a0d2e71e` and `7fe0e266` the decoder's sources, `bitstream/` and `pyrowave_common.*` are
identical, and so is the embedded SPIR-V of `idwt`, `idwt_vs`, `idwt_fs` and `wavelet_dequant`; the
SPIR-V that changes is the encoder's (`block_packing`, `analyze_rate_control*`,
`resolve_rate_control`, `wavelet_quant`). The stream format does not change, so this decoder stays
compatible with an encoder at the Rubylight host's `502a3b52`, and the desktop client's own pin
(`protocol/desktop/pyrowave-sdk.pin`) stays where it is. These libraries were built from upstream
archives of the same commits, with the options above.

| ABI | Bytes | SHA-256 |
| --- | ---: | --- |
| arm64-v8a | 1,249,048 | `488a4ad102d4475db787830df71e5d07b14ccdc8c8998c97b2d4a69df034434b` |
| x86_64 | 1,303,208 | `728d4a2a2179a076d9a89a5b2e79742ed6956bd65f8bec2e6230db61a9b90505` |

To regenerate `shaders_spv.h`, compile each `shaders/fullscreen.vert`, `shaders/planar_csc.frag` and
`shaders/sgsr.frag` with NDK `shader-tools/windows-x86_64/glslc.exe -O --target-env=vulkan1.1 -mfmt=c`, and put
those initializer lists in `static const uint32_t fullscreen_vert_spv[]` / `planar_csc_frag_spv[]` / `sgsr_frag_spv[]`.
Also compile binary SPIR-V and run `spirv-val.exe --target-env vulkan1.1` on it. The renderer runs on
Vulkan 1.1 devices, so the SPIR-V must stay at version 1.3 or lower.

### Exact real-device checks still required

Local verification passed with Windows JDK 17, the specified SDK, Gradle `--no-daemon --max-workers=2`,
two active processors and a 1.5 GiB heap: `assembleNonRootDebug`, `assembleNonRootRelease`,
`testNonRootDebugUnitTest` (46 tests, zero failures/errors/skips), and `lintNonRootDebug` (zero errors,
201 existing warnings). Both APKs contain all four ordinary-codec ABIs and the two 64-bit PyroWave
ABIs, with the complete notices preserved in release assets. Release 16 KiB zip alignment passed.
The native parser/RTSP assertions and the 240-frame milestone 1 replay passed; the standalone ARM64
benchmark also compiled against the bundled decoder. The root flavor and real-device checks below
were not run.

1. Build with JDK 17, the specified SDK, `gradlew.bat --no-daemon --max-workers=2
   assembleNonRootDebug testNonRootDebugUnitTest`. Native make is already capped at `-j2`.
   Find the APK under `app/build/intermediates/apk/nonRoot/debug/` with AGP 9.4, then
   `adb -s SERIAL install -r app/build/intermediates/apk/nonRoot/debug/app-nonRoot-debug.apk`.
2. On a Vulkan 1.3 Adreno and a Mali/Immortalis device, set Codec **Prefer PyroWave**,
   enable the performance overlay, and start at 1280×720/60. Pair with Rubylight built with its
   `186f0393` PyroWave encoder. Use adequate LAN bitrate (e.g. 200,000 kbps on gigabit Ethernet).
   Capture `adb -s SERIAL logcat -v threadtime` to a file during the run. Verify the PyroWave surface
   preparation and selected decode path logs, actual codec/chroma in the overlay, and host SDP
   `bitStreamFormat=3`. Check that neither record-mode attribute is sent.
3. Repeat with 4:4:4 on/off, HDR on/off, full range on/off, and host `prefer_sdr_10bit`. On an HDR10
   panel, display ten-bit grey ramps, black/white patches, PQ highlights and single-pixel red/blue
   text on the host. Confirm the actual SurfaceView's 10-bit ST2084 swapchain in Vulkan validation /
   SurfaceFlinger diagnostics, BT.2020 primaries, no eight-bit intermediate/banding, correct blacks,
   centre chroma and full-resolution colour text. Compare against a known-good host/local image.
   Change/remove mastering/MaxCLL/MaxFALL metadata; toggle host HDR while connected. Confirm no stale
   metadata, a new SDR colour-space swapchain on HDR-off and preserved ten-bit planes.
4. Pause/disconnect/resume, background/foreground, rotate, replace/destroy the surface, and stream
   while the host changes HDR. Verify objects and decode/presentation fences are released, no hangs
   or repeated invalid semaphore use, and no validation-layer errors. Apply temporary packet loss/
   reordering on a test network: complete recovered frames decode; unrecoverable frames drop and
   the next intra frame recovers. Feed truncated/bad packet lengths and duplicate short blocks through
   the native parser regression test; no loops or out-of-bounds reads are permitted.
5. Stop normally to finalize the milestone 1 CSV, then extract it using
   `adb -s SERIAL exec-out run-as com.butterpollo.client cat files/butterpollo-latency.csv > device.csv`.
   During streaming use `files/butterpollo-latency.csv.tmp`. Report input-to-output avg/p95/p99 and
   sample count, plus GPU-decode log/overlay timing separately. Render fields must remain blank with
   `render_unavailable`. Repeat 1080p/native resolution at 60/120/240 Hz where supported, including
   a thermal run; never substitute Windows timings for the device's measurements.
6. Repeat with a 32-bit/API <29/Vulkan <1.3 device, TextureView, explicit HEVC/AV1, a missing-feature
   Vulkan driver, and an HDR surface without the required format/extension pairs. Verify conventional
   fallback, including HDR preference over PyroWave SDR. On a test host remove/change the SDP bitstream
   ID and verify AV1/HEVC fallback; repeat with ordinary Sunshine/Apollo and H.264-only hosts.

For an on-device synthetic C API benchmark independent of a host, compile
`app/src/test/native/pyrowave_benchmark.cpp` with NDK's `aarch64-linux-android29-clang++.cmd`,
`-std=c++17 -O2 -static-libstdc++`, include `jni/pyrowave-renderer/prebuilt/include`, and link with
`-Lapp/src/main/jni/pyrowave-renderer/prebuilt/arm64-v8a -lpyrowave-shared`.
Push the executable and the matching `.so` into `/data/local/tmp/bp-pyrowave/`; run
`adb -s SERIAL shell 'cd /data/local/tmp/bp-pyrowave && chmod 700 benchmark && LD_LIBRARY_PATH=. ./benchmark > measurements.csv'`.
Pull `measurements.csv` and feed it to the Java adapter above, then remove that temporary directory.
This measures complete GPU decode plus readback; the live-stream CSV is the Android Surface renderer
measurement. No instrumentation/emulator or mobile performance result is claimed from this run.
