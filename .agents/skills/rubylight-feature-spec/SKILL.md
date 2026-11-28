---
name: rubylight-feature-spec
description: >-
  Use when writing or reviewing a Rubylight or Nightjar feature design spec
  before implementing non-trivial architectural or protocol changes.
---

# Rubylight Feature Spec Procedure

Follow `docs/encoder-focus-design.md` as the engineering depth bar and pair with narrow workflow skills in `.agents/skills/` when applicable (`add-control-message`, `add-native-endpoint`, `add-setting`, `check-nvenc`, `isolated-e2e`, `release-prep`).

## Required Spec Sections

Every spec under `docs/designs/<NN>-<slug>.md` should cover:

1. **Summary & User Experience**: Plain-language user value, zero internal jargon, zero "Experimental" labels, zero mod names (GPL-3.0 §7(e)).
2. **Protocol / Native API / Capability Contract**:
   - Preserve `PROTOCOL_VERSION` and stock Moonlight compatibility (clients asking for nothing new see nothing new — `tests/host/tests/reconnect_tests/extension_tests.rs`).
   - Gate all client extras via `HostCapabilities` (`client/protocol/policy/src/http/capabilities.rs` + Java `HostCapabilities.java`), never host brand or version string.
   - Reserve control IDs, input magics, serverinfo fields, launch params, and native feature strings in `docs/designs/00-protocol-reservations.md` first (`client/protocol/core/src/ids.rs`, `client/protocol/native-api/src/`).
3. **Architecture & Layering**:
   - Keep pure decisions in `rust/core/{crypto,config,media-policy,input-policy,session}/` (`butterpollo-*`) and `client/protocol/policy/` (`nightjar-policy`); Windows/GPU calls in `rust/windows/{abi,graphics,display,io}/` (`butterpollo-win-*`); host wiring in `rust/host/{stream,webrtc,web}/` (`butterpollo-{stream,webrtc,web}`); desktop split across `client/protocol/desktop-{config,ui,media}/` + thin `client/protocol/desktop/`.
   - Respect `tools/arch_check.py`: start every new Rust module with `//!` and every new Java/Kotlin file with `/**`; keep modules cohesive without artificial line-count splitting; never add raw `Config` reads in `rust/host/stream/src/stream(.rs|/**)` outside `stream/config.rs`; annotate `cfg(unix)` OS intent in desktop crates.
   - Use loopback (`127.0.0.1`/`[::1]`) and IP-helper tables in unit tests and port probes (no wildcard `0.0.0.0`/`[::]` listens).
   - Keep AMD AMF, NVIDIA NVENC (verified against `rust/windows/abi/include/nvEncodeAPI.h` & samples, never claimed as run on hardware), Windows 10, physical displays, WGC, Desktop Duplication, and D3D11 working.
4. **State Machine, Reconnect Matrix & Crash-Restore Journal**:
   - Define state transitions, reconnect/resume/roam/second-device cases, and lock order.
   - Journal any mutation to host PC state (displays, DPI, audio routes, frame limiter, process suspend state, files) before applying and restore on every exit, crash included (`RecoveryJournalV2`).
5. **Settings & Craft**:
   - Declare host settings in `rust/core/config/settings.toml` (`python tools/generated_docs.py`) and client settings in `client/protocol/policy/settings.toml` (`python client/scripts/generate-settings.py`).
   - Prefer safe measured defaults over extra settings; plain-English errors; dynamic input glyphs; zero stream-time blur; zero latency added to the default path.
6. **Disjoint Slices & Verification Plan**:
   - Partition parallel work into non-overlapping file lists, and name the narrowest unit, interop, visual, and hardware checks that prove the change.
   - Before coding, verify target paths against live `docs/code-map.md` and `client/docs/code-map.md`, skip already-landed items, and update the code maps when modules are added or moved.
