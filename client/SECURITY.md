# Security policy

## Supported versions

Security fixes go into the latest Nightjar release (currently 4.0.x) on Android and desktop. Older releases don't get them, so please update before reporting.

## Reporting a vulnerability

Please don't open a public issue. Report it privately on GitHub: **Security → Report a vulnerability** on this repository, or go straight to [the private report form](https://github.com/RamazanKara/nightjar/security/advisories/new).

Tell me what you found, the Nightjar version and device, how to reproduce it and what an attacker gains. A client log helps; remove anything private first.

You'll usually hear back within a week. The fix ships in a release, then the advisory is published, with credit if you want it.

## Scope

In scope: the Nightjar apps for Android and desktop, meaning pairing, the stored client certificate and keys, the stream and control connections, update checks and anything the app stores on the device.

Out of scope: problems that need root, an unlocked device or a debugger attached to the app, and problems in a host. Report host problems to the host's project; for Rubylight, see [its security policy](https://github.com/RamazanKara/Rubylight/security/policy).
