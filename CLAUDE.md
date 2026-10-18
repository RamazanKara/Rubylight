# Nightjar

Host-neutral Android Moonlight-protocol client (repo RamazanKara/nightjar). Works with Sunshine, Apollo, Vibepollo, Rubylight and GFE.

- Host-specific extras are gated by the `HostCapabilities` model, never by brand checks. User-facing text says Nightjar and stays host-neutral.
- Never change applicationId `com.butterpollo.client` or the signing key: in-place updates depend on them.
- Never rename existing preference keys (`SettingsKeysTest` enforces this).
- Native moonlight-common-c fixes live in `moonlight-core/patches/*.patch`; Gradle and `test-native.sh` reapply the set when its stamp changes.

## Release signing
Keystore and `keystore.properties` live in `C:\Users\ramaz\git\signing\nightjar\`. The former `C:\Users\ramaz\.keys\nightjar\` path is a compatibility junction. For a release build, copy `keystore.properties` to the repo root (gitignored), build, delete it again. Verify with `apksigner verify --print-certs`: SHA-256 must be `312ed322c7c29d1aec4fae4a3979ebcb557af44d821d7f4746c0968548eb630b`. Never print or commit the properties file.
