# Security policy

## Supported versions

Security fixes go into the latest Rubylight release (currently 3.0.x). Older releases, 2.6.x included, don't get them, so please update before reporting.

## Reporting a vulnerability

Please don't open a public issue. Report it privately on GitHub: **Security → Report a vulnerability** on this repository, or go straight to [the private report form](https://github.com/RamazanKara/Rubylight/security/advisories/new).

Tell me what you found, the Rubylight version, how to reproduce it and what an attacker gains. A host log (`C:\ProgramData\Butterpollo\config\logs\butterpollo.log`) helps; remove anything private first.

You'll usually hear back within a week. The fix ships in a release, then the advisory is published, with credit if you want it.

## Scope

In scope: the Rubylight host, meaning pairing, the web UI, the HTTP, RTSP and stream endpoints, the installer, the service and the virtual display driver.

Out of scope: problems that need a paired client or admin rights on the PC and give nothing beyond that, and denial of service from the local network. Rubylight is meant for your own network; don't expose its ports or web UI to the internet. Problems in a client or another host belong to that project; for Nightjar, see [its security policy](https://github.com/RamazanKara/nightjar/security/policy).
