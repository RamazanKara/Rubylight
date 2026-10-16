# Code signing policy

Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).

Rubylight's Windows releases are signed through the SignPath Foundation's programme for open-source projects, starting with the first release published after the project's SignPath setup is complete. Earlier releases are unsigned. A signed file names **SignPath Foundation** as its publisher.

## Team roles

Rubylight has one maintainer, who holds every role SignPath asks a project to assign.

| Role | What it means | Members |
| --- | --- | --- |
| Committers | May change the source code in the repository without another review. | [Ramazan Kara (RamazanKara)](https://github.com/RamazanKara) |
| Reviewers | Review every change proposed by someone who is not a committer, such as a pull request, before it is merged. | [Ramazan Kara (RamazanKara)](https://github.com/RamazanKara) |
| Approvers | Approve each signing request in SignPath, deciding whether a build may be released as signed. | [Ramazan Kara (RamazanKara)](https://github.com/RamazanKara) |

Team members use multi-factor authentication for GitHub and for SignPath. When the team grows, this table is updated before a new member gets any of these roles.

## What is signed

Only files built from this repository's source code by its GitHub Actions workflow ([rust-windows.yml](../.github/workflows/rust-windows.yml)) are signed:

- Rubylight's executables and DLLs: the host, its service and start programs, setup (also installed as `uninstall.exe`), the diagnostic probes and the Vulkan HDR layer.
- The installer that carries them, `rubylight-setup-<version>.exe`.
- Rubylight's own virtual display driver: its DLL and catalog.

Every signed executable and DLL carries the product name **Rubylight** and the release's version, and SignPath refuses files that don't ([artifact configurations](../.signpath/)). The [code signing guide](code-signing.md) describes the setup.

Files from other projects keep their own publishers' signatures and are never signed with Rubylight's certificate: the virtual display and gamepad drivers released with Vibepollo (signed by SignPath Foundation for that project), the usbip-win2 installer and drivers and `nefconc.exe`. Open-source libraries whose projects publish no signed builds ship unsigned: the Opus DLL from MSYS2 and the PyroWave library built from its pinned source. [Third-party components](../rust/THIRD_PARTY.md) lists them all with their licences.

## Certificates the installer trusts

Setup adds certificates to Windows' certificate stores on the PC it installs on. Its first page says so before anything is installed, and uninstalling removes them.

- **Rubylight's virtual display driver.** Windows installs a driver only from a publisher it trusts. Until SignPath signs the driver's catalog, `install-local.ps1` creates a code-signing certificate on the PC, signs the catalog with it, adds the certificate to `LocalMachine\Root` and `LocalMachine\TrustedPublisher`, and deletes its private key, which never leaves the PC and cannot be exported. With the key gone, that certificate can sign nothing else, for Rubylight or for anyone else. Every PC trusts only its own. A catalog signed by SignPath is installed as it is: only its SignPath Foundation certificate goes into `TrustedPublisher`, so Windows does not ask before installing the driver. Reinstalling replaces the previous certificate, and uninstalling Rubylight removes the certificates and the driver even when the other drivers stay. ([Driver README](../rust/drivers/rubylight-vdisplay/README.md#shipping-before-signpath-signs-it-signed-on-the-users-pc))
- **The local web console.** So browsers open `https://localhost` without a warning, the host makes a root certificate named "Rubylight local console" on first start, uses it to issue the console's own certificate, and discards its private key. Setup adds that root to `LocalMachine\Root`. It is restricted to `localhost`, `127.0.0.0/8` and `::1`, may not issue further certificate authorities, and cannot vouch for any other site. Uninstalling removes it.

Drivers from other projects that setup installs come with their own install scripts, which may trust their publishers' certificates in the same way.

## How a release is signed

1. Pushing a version tag (`v2.*`), or the maintainer starting the workflow's release on `main`, runs the Windows workflow on GitHub-hosted runners. It builds and tests everything from that commit, without caches from earlier runs.
2. The workflow uploads Rubylight's own binaries to SignPath. SignPath checks with GitHub that the build came from this repository's workflow and commit, then waits for an approver.
3. The approver checks that the request belongs to the release being published and approves it. The workflow puts the signed files in the package, builds the installer, and sends the installer for signing the same way.
4. The workflow checks every signature and timestamp before it publishes the release. If any signing step fails, nothing is published, signed or unsigned.

Pull requests and ordinary pushes to `main` are never signed. Changes to build scripts and the workflow are reviewed like any other source code, because they decide what gets signed.

## Privacy

Rubylight streams to the devices you pair and announces itself on your local network so Moonlight can find it. Otherwise it transfers information to other networked systems only when the user or the person installing or operating it asks for it, with one exception that is on by default: when the host starts and then once a day, it asks GitHub whether a newer release exists. That request carries no information about you or your PC beyond the IP address any connection reveals, and `update_check_interval = 0` turns it off. The [privacy statement](privacy.md) lists every connection Rubylight makes, what each one sends, how to turn it off, and the privacy policies of the services involved.

## Check a download

In Explorer, right-click the installer, choose **Properties** and open **Digital Signatures**: the signer is SignPath Foundation, with a timestamp. In PowerShell:

```powershell
Get-AuthenticodeSignature .\rubylight-setup-<version>.exe | Format-List Status, SignerCertificate, TimeStamperCertificate
```

Each release also lists the SHA-256 of its files in `SHA256SUMS`.

## Reporting a problem

If you think a file signed for Rubylight breaks this policy, open an [issue](https://github.com/RamazanKara/Rubylight/issues) or tell SignPath at support@signpath.io.
