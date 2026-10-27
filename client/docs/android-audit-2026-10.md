# Android audit — 13 October 2026

[Documentation](index.md) · [Host feature matrix](PARITY-MATRIX.md) · [Building](building.md)

## Scope

Reviewed Android `main` after 0.4.5: documented host parity, pairing/network lifecycle, per-PC settings and deletion, GPU-driver import, launcher export, tests, documentation and release helpers. There were no open Android issues or pull requests when the audit began. Closed pairing issue #2 still makes real-device retesting valuable; its 0.4.5 regression fix is already present.

The source is prepared as 0.4.6 (`versionCode` 323). This version bump is not evidence of publication; use the repository's Releases page for published binaries.

## Defects and missing safeguards addressed

| Finding | Change | Regression coverage |
| --- | --- | --- |
| Renaming a host app created a second launcher entry instead of updating the existing app. | Match existing entries by PC/app identity and preserve filenames, including `.ART` casing and matching cover names. | Storage Access Framework document-provider tests. |
| A failed new entry write left an empty `.art` file; retries generated suffixed duplicates. | Best-effort removal of newly created incomplete documents; existing entries are never deleted on a write failure. | Failed-write and retry tests. |
| A failed cover copy left an empty PNG that prevented later exports from retrying it. | Remove incomplete new covers when the provider permits deletion; keep existing custom covers. | Cover failure/retry and preservation tests. |
| Removing a PC retained its cached app list and empty artwork directory; hidden-app cleanup depended on the UI caller. | Put PC-scoped cache/hidden-app cleanup in the computer service with the existing per-PC preference cleanup. | Removed-PC and unaffected-PC assertions. |
| Release helpers produced an ARM64-only APK despite universal published releases, and skipped validation. | Build all default ABIs after release lint and both flavors' unit tests pass. | Shell execution with a fake Gradle launcher, failure gate and PowerShell static parity checks. |
| Common PKCS#12 signing-key filenames were not ignored. | Ignore `.p12` and `.pfx` alongside the existing keystore patterns, and document secret handling. | Git ignore-rule checks. |
| Documentation-only changes skipped the only docs CI job. | Add a dedicated documentation workflow for relevant pushes and pull requests. | Generated-settings and local-link checks. |
| The bug template required GeForce Experience details and implied gamepad testing for pairing failures. | Ask for exact client/host versions and failure stage; add Android-client and not-applicable/not-tested choices. | Template review against the deficiencies recorded in issue #2. |

The four core regressions fail against the pre-fix production code and pass with the fixes.

## Validation of the prepared version

Executable source at `c7964a89b9bf2ff16bf17aa5ed6d303115060208` passed the following checks; subsequent audit/store-description edits are documentation only:

- **Android unit tests:** 846 non-root and 851 root tests passed (1,697 total; no failures, errors or skips).
- **Builds and lint:** non-root debug APK, both minified release APKs and the non-root release bundle built successfully. Each release lint report has zero errors and 213 warnings; the warnings are not claimed to be resolved.
- **Packaging:** both universal release APKs contain all four default ABIs, pass signature verification and 16 KB ZIP alignment checks, and have native ELF load segments aligned to at least 16 KB. The bundle signature also verifies. These artifacts use a disposable validation key, not the production signing identity.
- **Native validation:** C tests passed normally and with AddressSanitizer/UndefinedBehaviorSanitizer; 19 Rust tests and C++ framing/presentation tests passed. Eight differential seeds covered 640,000 identical C/Rust frames.
- **Hostless Android 35 emulator:** the minified non-root release APK passed fresh-install, pairing-fixture, host-profile, settings, controller-mapping, reset and frontend-entry checks at 393/412 dp and font scale 1.3. All three removed-PC checks deleted only that PC's app-list/artwork fixtures and preserved another PC's files. No app crash was found. This is not live streaming or a production-key upgrade test.
- **Documentation/release helpers:** all eight Node tests pass; generated settings match source and local documentation links resolve. External links are syntax-checked only. PowerShell parity is checked statically, not executed on Windows.
- **Hosted CI:** [Android build and test run](https://github.com/RamazanKara/nightjar/actions/runs/38317358931) and [documentation run](https://github.com/RamazanKara/nightjar/actions/runs/38317358741) passed.

**Publication is blocked:** the retained release key/configuration was unavailable during this audit. No `v0.4.6` tag or release was created, and no validation-key artifact was published. The exact production-signed APK still needs certificate comparison with 0.4.5 and an in-place upgrade check before publication.

## Still missing or not established

- **Physical-device validation:** live host streaming, custom Snapdragon Vulkan drivers, HDR/VRR, USB DualSense feedback, microphone/clipboard and end-to-end latency need real hardware. Hostless JVM/emulator checks do not establish these.
- **Pairing verification:** retest the affected Galaxy Z Fold6 and Odin 2 Portal against a live host. The original issue was closed; that does not substitute for those device checks.
- **Launcher integration:** real ES-DE/Daijisho/Pegasus and third-party/cloud document-provider behavior need an integration pass. Automatic pruning of previously exported games that are now removed or hidden is not implemented; it must not silently delete user files. Existing documents cannot be updated transactionally on every Storage Access Framework provider.
- **Release signing and upgrade:** a stable update requires the retained release key and a certificate comparison with 0.4.5. Disposable validation keys must never be published as replacement releases. Validate an in-place upgrade of the exact signed APK before marking it Latest.
- **Host administration:** device management, app overrides and host services deliberately remain in the host web console. They are not partially implemented Android screens.
- **Store distribution:** Play Console enrollment/submission and the reviewed Data safety declaration remain owner steps. German and tablet store screenshots are still missing; see [store readiness](../internal/STORE-READINESS.md). Historical audit evidence remains in [the internal parity log](../internal/BUTTERPOLLO_PARITY.md); use the current host feature matrix and [device checklist](../internal/device-checklist.md) for today's behavior.

No new performance claim or blanket "all features complete" claim follows from this audit.
