# AMD AMF headers

The subset of AMD's Advanced Media Framework (AMF) C headers that the AMF
bindings in `rust/windows/build.rs` include: `core/Factory.h`, the H.264,
HEVC and AV1 encoder components, and the headers those pull in.

- Source: [GPUOpen-LibrariesAndSDKs/AMF](https://github.com/GPUOpen-LibrariesAndSDKs/AMF),
  `amf/public/include`
- Version: 1.5.2.0 (`AMF_VERSION_*` in `core/Version.h`)
- License: MIT, Copyright (c) Advanced Micro Devices, Inc. Each file keeps
  AMD's license header.

The files are unmodified apart from line endings (LF). To update them, copy
the same files from a newer AMF release and keep this list complete: the
bindings fail to generate if one of their includes is missing.
