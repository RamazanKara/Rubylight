# Code signing policy

[Docs](README.md) · [Download Rubylight](https://github.com/RamazanKara/Rubylight/releases) · [Build guide](building.md)

Rubylight is a Windows game-streaming host by [RamazanKara](https://github.com/RamazanKara), licensed under [GPL-3.0-only](../LICENSE). The [upstream credits and notices](../NOTICE) and [third-party licenses](../rust/THIRD_PARTY.md) remain part of its distribution.

**Status:** We are preparing an application to SignPath Foundation. Signing is disabled until the maintainer configures the account and release policy. This page does not claim that the application has been accepted or that existing downloads are signed. Check an EXE's **Properties → Digital Signatures** for that particular download.

The attribution for signing once approved is:

> Free code signing provided by SignPath.io, certificate by SignPath Foundation

[SignPath.io](https://signpath.io) provides the signing service; [SignPath Foundation](https://signpath.org) provides the certificate.

## Team

| Role | Member | Responsibility |
| --- | --- | --- |
| Authors | [RamazanKara](https://github.com/RamazanKara) | Maintain the source and build scripts. |
| Reviewers | [RamazanKara](https://github.com/RamazanKara) | Review contributions before release. |
| Approvers | [RamazanKara](https://github.com/RamazanKara) | Review and manually approve each signing request. |

All team members must use MFA for GitHub and SignPath. The maintainer must verify this before enabling signing.

## Signed files and exclusions

Once enabled, the release pipeline signs only Rubylight's own PE files built from this repository: the host (including its session/helper modes), Windows service, launcher, three packaged performance probes, Vulkan HDR layer and assembled installer. The installed uninstaller retains the complete signed installer so its signature remains valid. Each signed file carries ProductName **Rubylight** and ProductVersion equal to the Cargo release version, including any prerelease suffix. SignPath enforces both fields.

The package also contains upstream open-source components. MSYS2 GNU runtime, Opus, oneVPL and PyroWave/Granite DLLs, the Playnite plugin and upstream driver tools are not signed with Rubylight's certificate. Existing upstream signatures are preserved. The virtual display and gamepad driver packages retain their SignPath Foundation signatures from the pinned Vibepollo release, which the build checks. Windows and installed graphics-driver libraries remain system dependencies.

The standard release excludes both `butterpollo_truehdr.dll` and `nvngx_truehdr.dll`: the optional adapter links NVIDIA's proprietary NGX import library, and the runtime uses the NVIDIA RTX Video SDK license. The [optional unsigned TrueHDR add-on](building.md#optional-unsigned-truehdr-add-on) is built and installed separately, is never submitted to SignPath and is not covered by this signing policy. Its NVIDIA license must accompany it.

## Build and approval

[Rubylight Windows](../.github/workflows/rust-windows.yml) runs on GitHub-hosted Ubuntu and Windows runners. With signing enabled, a release builds from its own checked-out commit; it does not reuse artifacts from another workflow run. Formatting, tests, Clippy and the web console checks precede packaging with `-SkipTrueHdr`.

The signing job uploads the unpacked package as a GitHub Actions ZIP artifact, submits the explicitly listed Rubylight files, waits for approval, then rebuilds the file manifest and portable ZIP from the signed files. It rebuilds the installer around that payload and submits the installer in a second request. Both requests use the manually approved `release-signing` policy. The workflow verifies signatures and extracts/verifies the signed installer before publication. Release `SHA256SUMS` is computed from the final installer and portable ZIP.

An absent `SIGNPATH_ORGANIZATION_ID` leaves the existing unsigned release path active, including reuse of a successful build for the same commit. A configured signing path that fails cannot publish unsigned fallback assets.

## Privacy

Rubylight sends streams, audio, input feedback and requested host information to clients the operator pairs and permits. There is no analytics service or automatic upload of logs or crash reports. Profiles, paired-device identities, credentials, covers, logs and local crash diagnostics are stored on the host; support reports are shared only when the operator chooses to share them.

Outbound traffic found in the host and console source includes:

| Activity | Destination and data | Control |
| --- | --- | --- |
| Scheduled or manual update checks | GitHub release API; GitHub receives the source IP address, request and `Rubylight` user agent. | Scheduled checks default to once per 86400 seconds. Set `update_check_interval = 0` to disable them. Manual checks remain available. |
| Installer downloads | GitHub and its release-asset hosts receive normal HTTPS download requests. | Manual installation, or `auto_update` when explicitly enabled; automatic installation defaults off. |
| Steam artwork | Valve's Steam CDN receives game IDs in image URLs, source IP address and the artwork user agent when missing or small local covers are replaced. | `steam_enabled` and `steam_auto_sync` default off. Enabled automatic sync can download covers in the background. Set `steam_auto_sync = false` to stop that background work, or `steam_enabled = false` to disable the integration. |
| Cover search and selection | The browser requests GitHub-hosted LizardByte GameDB search buckets and IGDB image previews. The host downloads a selected IGDB cover. These services receive the requesting device's IP and requested search/image paths. | Initiated through the operator's cover search/selection; local PNG uploads are available. |
| Local discovery and optional port mapping | mDNS advertises the host name and service on the local network. When `upnp` is enabled, SSDP/UPnP contacts the local router for port mappings or IPv6 pinholes. | Host discovery is part of running a streaming server. UPnP defaults off; stop the host to stop its discovery traffic. |
| Hardware metrics | CPU/GPU readings are returned only to an authenticated paired client with permission and an active video stream that requests them. | `hardware_metrics` defaults off; see [Hardware metrics](hardware-metrics.md). |

Third-party services apply their own policies: [GitHub privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement), [Valve privacy policy](https://store.steampowered.com/privacy_agreement/) and [Twitch/IGDB privacy notice](https://www.twitch.tv/p/en/legal/privacy-notice/). Visiting external links or running operator-configured applications/preparation commands can also make connections according to those applications' settings.

Interactive setup displays this network disclosure and offers **Disable background downloads** before starting the host. It sets `update_check_interval = 0`, `auto_update = false` and `steam_auto_sync = false`, preserving other settings. Quiet installations retain the existing profile; administrators can set those same keys in `%PROGRAMDATA%\Butterpollo\config\sunshine.conf` and use `--no-start` while configuring a new installation.

## System changes and removal

Setup discloses its automatic Windows service (`ApolloService`), inbound firewall rules, virtual display/gamepad drivers and Vulkan HDR implicit layer registration. The Vulkan loader sees the layer in all Vulkan apps; HDR activation is limited to Rubylight's owned sessions. The service registers it machine-wide; direct hosts can register it for the current user. Display changes, virtual monitors and default-audio-device switching during streams are restored by the existing session shutdown/recovery paths, without overwriting subsequent user changes.

Rubylight deliberately supports lock-screen and secure-desktop streaming for permitted paired clients. The operator installs and authorizes the service and controls pairing; Windows sign-in and UAC authentication still apply. Do not pair untrusted clients.

Use **Windows Settings → Apps → Rubylight → Uninstall**. Uninstallation stops the service and streams, removes the service, firewall rules, installed files and the installation's Vulkan registry values in the machine and loaded user hives. It preserves settings and shared drivers by default. Choose **Remove drivers too** (or `uninstall.exe --uninstall --remove-drivers`) to run the bundled driver removal scripts; restart Windows if needed. The dialog can also delete settings, paired devices and logs (`--factory-reset`). Driver removal failures are reported before their scripts are deleted. A per-user Vulkan registration in an unloaded Windows profile may need removal from that user's `Software\Khronos\Vulkan\ImplicitLayers` key after sign-in.

## Maintainer activation checklist

1. Verify MFA on every team member's GitHub and SignPath accounts and apply to the Foundation.
2. Install the SignPath GitHub App with access to this repository and connect GitHub.com as the trusted build system for the `rubylight` project.
3. Register [.signpath/artifact-configuration.xml](../.signpath/artifact-configuration.xml) as `payload` and [installer-artifact-configuration.xml](../.signpath/installer-artifact-configuration.xml) as `installer`. Both use the required `version` parameter and a GitHub artifact ZIP as their root.
4. Create `release-signing` with the Foundation certificate, trusted hosted builds, restricted release refs and **manual approval required for every request**. Assign the team above. The workflow cannot establish this server-side approval rule.
5. Add the submitter token as the repository secret `SIGNPATH_API_TOKEN`. Only after the project and both artifact configurations are ready, set the repository variable `SIGNPATH_ORGANIZATION_ID`. Its presence enables signing for releases; ordinary CI builds remain unsigned.
6. For each release, review the source commit, build result, version, artifact contents and exclusions; approve the payload request, then the assembled-installer request. The workflow waits up to one hour for each approval. Validate the first live signed build with SignPath before describing published downloads as signed.

These steps follow the [Foundation terms](https://signpath.org/terms), [GitHub trusted-build integration](https://docs.signpath.io/trusted-build-systems/github) and [artifact metadata restrictions](https://docs.signpath.io/artifact-configuration/reference#file-metadata-restrictions).

## Application draft

Rubylight is a free GPL-3.0-only Windows game and desktop streaming host for Moonlight-compatible clients, maintained by RamazanKara. It is written in Rust, focuses on AMD Radeon capture/encoding latency and includes a local browser administration console.

- Repository: https://github.com/RamazanKara/Rubylight
- Releases and downloads: https://github.com/RamazanKara/Rubylight/releases
- Project website: https://ramazankara.github.io/Rubylight/

We request signing for our own host/helper executable, service, launcher, performance probes, Vulkan HDR DLL and installer/uninstaller. GitHub Actions builds them from the release commit on GitHub-hosted runners. The pipeline signs the payload before reassembling and signing the installer, and waits for a maintainer's manual approval of both requests. Upstream DLLs retain their own signatures or remain unsigned. Already-signed virtual display/gamepad drivers are redistributed unchanged. The proprietary NVIDIA TrueHDR runtime and NGX-linked adapter are excluded from the signed distribution and offered only as a separately built, unsigned optional add-on.

Reputation evidence, checked through the GitHub API on October 12, 2026: the repository had **56 stars** and **2613 release-asset downloads** across its published releases. Asset downloads include installers, ZIPs and validation files and are not a count of distinct users. Please refresh these figures when applying. Public issues, release history and [reproducible performance/validation records](../rust/PERFORMANCE.md) provide additional evidence. Rubylight was formerly Butterpollo and began in the Vibepollo → Apollo → Sunshine lineage; the earlier C++ implementation was replaced by the Rust host. Existing upstream attribution and licenses remain in NOTICE and the third-party notices. This lineage is context, not a claim that upstream signing approval transfers to Rubylight.

Upfront feature disclosure: Rubylight can stream and control the lock screen and secure desktop, as user-installed Sunshine/Apollo-family streaming hosts do. This is an intentional remote-use feature of an operator-installed service, limited to paired clients with permissions. It does not remove Windows sign-in or UAC requirements and is not intended to bypass security. The installer and policy disclose this behavior for the Foundation's review.
