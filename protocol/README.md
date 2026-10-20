# rubylight-protocol

The wire formats Rubylight's host and clients share, written once in Rust.

These are the Rubylight host's extensions, so the crate, its C header and the static
library keep their `rubylight` names: the host pins this crate, and Nightjar uses them only
when the host advertises them.

- `core/` is the `rubylight-protocol` crate: `no_std` with `alloc` and no dependencies, so
  the host can take it as an ordinary dependency.
- `ffi/` wraps it in a C ABI for the Android client (`ffi/include/rubylight_protocol.h`).
- `tests/differential.sh` compares it with the C++ framing the client used before
  (`tests/frame_reference.h`) on random, lossy and corrupted frames.

It holds:

- `pyrowave`: the client side of PyroWave framing, the packet container and the record
  reassembly that lets a frame with lost packets still decode.
- `phase_lock`: the client's report of how early frames are ready before its display latch,
  and the host controller that times frames to land just before it.

## Building for Android

The app links prebuilt static libraries of this crate from
`app/src/main/jni/protocol/prebuilt/`. After changing the crate, refresh them:

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
protocol/build-android.sh      # runs the tests, then rebuilds all four ABIs
protocol/tests/differential.sh # optional: 8 seeds x 2000 streams against the old C++
```

Only the crate's own object goes into each archive (about 25 KB); it needs nothing but libc.

The Rust streaming engine is different: `android/` (`nightjar-android`) builds
`libnightjar_core.so`, the JNI bridge over `nightjar-core` behind the app's "New streaming
engine" setting. It links Rust's std, ring and the app's libopus, so it is not committed:
the Gradle task `buildRustEngine` (app/build.gradle) runs cargo for each ABI with the NDK's
clang as compiler and linker and checks the 16 KiB segment alignment. A normal app build
therefore needs rustup with the four targets above. See internal/RUST_CORE.md.
