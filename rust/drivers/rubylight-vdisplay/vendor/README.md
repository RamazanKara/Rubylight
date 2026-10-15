# Vendored windows-drivers-rs

`wdk-build` and `wdk-sys` are microsoft/windows-drivers-rs 0.5.1
(https://github.com/microsoft/windows-drivers-rs), Copyright (c) Microsoft Corporation,
MIT OR Apache-2.0, as vendored by punktfunk (Enrico Bühler (unom), MIT OR Apache-2.0), which
added an `iddcx` API subset: bindgen over `iddcx/1.10/IddCx.h` (wdk-build `ApiSubset::Iddcx`,
wdk-sys feature `iddcx` and `src/iddcx.rs`). Released 0.5.1 has no IddCx bindings.

The driver workspace redirects every `wdk-sys` and `wdk-build` dependency here through
`[patch.crates-io]`. Do not chase upstream without re-checking the IddCx subset.
