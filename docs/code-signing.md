# Windows release signing

Rubylight's Windows releases can be signed through the [SignPath Foundation](https://signpath.org), which gives open-source projects free code signing with a certificate issued to the Foundation. The workflow, the artifact configurations and the public [code signing policy](code-signing-policy.md) are in this repository. Signing stays off until the SignPath project exists and its secret and variables are set in GitHub; until then releases are unsigned, exactly as before.

This page is for the maintainer. It describes what the workflow does, then lists every step on SignPath's side and GitHub's, in order, and ends with the text for SignPath's application form.

## What the workflow does

[rust-windows.yml](../.github/workflows/rust-windows.yml) signs only release runs: a pushed `v2.*` tag, or **Run workflow** on `main` with **publish**. It signs nothing while the `SIGNPATH_PROJECT_SLUG` variable is empty. Pull requests and ordinary pushes to `main` are never signed, because the Foundation's release policy asks an approver to approve every signing request.

A signed release:

1. Builds and tests the tagged commit afresh, without the dependency cache, instead of reusing `main`'s unsigned build.
2. Checks that every Rubylight binary names the product **Rubylight** and the release version (every build runs this check, signed or not). SignPath refuses files whose version information doesn't match.
3. Uploads Rubylight's own executables and DLLs as a GitHub artifact and submits them with the [`binaries`](../.signpath/binaries.xml) artifact configuration. The request passes the release version as the `version` parameter and waits up to an hour for the approval.
4. Checks every returned signature and timestamp, puts the signed files in the package and regenerates its SHA-256 manifest.
5. If the package carries Rubylight's own display driver, catalogs it and signs the catalog with the [`driver`](../.signpath/driver.xml) configuration. See [Rubylight's display driver](#rubylights-display-driver).
6. Builds the ZIP and the self-extracting installer, submits the installer with the [`installer`](../.signpath/installer.xml) configuration, and checks its signature.
7. Publishes the release only if all of that succeeded. A configured signing request that fails, times out, lacks a file, or returns an invalid or untimestamped signature fails the build; it never publishes unsigned binaries.

A signed release therefore needs two approvals, three once the driver ships. The first request arrives when the build finishes, roughly 35 to 45 minutes after the run starts, and each later one a few minutes after the approval before it. The job allows four hours in total.

Third-party drivers and DLLs keep their upstream signatures and are never submitted. The signed setup executable inside the package becomes `uninstall.exe`; removing an outer installer's appended payload would invalidate its signature. The payload reader recognizes the PE certificate table and alignment padding, so the appended installer signature does not hide the package footer.

## Checklist

Do these in order. Steps 1 to 5 can happen before applying; steps 7 to 18 need the accepted SignPath account.

### Before applying

1. **Only open-source code: done.** Rubylight removed its NVIDIA TrueHDR adapter, the one part that linked a proprietary library (NVIDIA's NGX) and shipped NVIDIA's `nvngx_truehdr.dll`. Every build, signed or not, now comes from this repository's open-source code.
2. **Turn on two-factor authentication on GitHub** (Settings, Password and authentication). The Foundation requires multi-factor authentication for the repository and for SignPath.
3. **Protect `main` and the release tags.** In the repository's Settings, Rules, Rulesets, add a branch ruleset for `main` and a tag ruleset for `v*`, both active with **Restrict deletions** and **Block force pushes** (for tags also **Restrict updates**). Direct pushes to `main` keep working. SignPath's origin check trusts the branch or tag name, so these refs must not be rewritable.
4. **Publish the policy pages.** Merge this work to `main`; the Pages workflow publishes [the code signing policy](https://ramazankara.github.io/Rubylight/docs/code-signing-policy.html) and [the privacy statement](https://ramazankara.github.io/Rubylight/docs/privacy.html), and the landing page's install section shows the SignPath attribution. Check all three load. The next release's notes link the policy too.
5. **Tell users about system changes in the installer: done.** The Foundation asks software to warn before changing system configuration, and to show a privacy policy during installation when it sends user data to systems the user didn't choose. Setup's confirmation summary (`SYSTEM_CHANGES` in [detect.rs](../rust/setup/src/detect.rs)) names the service, the firewall rule, the virtual display driver, the two certificates it trusts and the daily update check, and links the privacy statement. The [code signing policy](code-signing-policy.md#certificates-the-installer-trusts) describes the certificates.

### Apply

6. **Fill in the form at [signpath.org/apply](https://signpath.org/apply)** with the [application text](#application-text) below. Tick the box accepting the SignPath Foundation Code of Conduct (the conditions in [signpath.org/terms](https://signpath.org/terms)) and the one allowing SignPath to store your data. SignPath reviews the application and, if it accepts, gives you access to a SignPath organization for Rubylight; its acceptance e-mail has the details.

### In SignPath, after acceptance

7. **Secure your account.** Sign in at [app.signpath.io](https://app.signpath.io) and turn on multi-factor authentication in your user profile.
8. **Check the project.** Open the Rubylight project (create it if the Foundation hasn't). Set **Repository URL** to `https://github.com/RamazanKara/Rubylight` and note the project's **slug**.
9. **Connect GitHub.** On **Trusted Build Systems**, choose **Add predefined**, add **GitHub.com** to the organization, then open the project and **Link** it under **Trusted build systems**. Optionally install the [SignPath GitHub App](https://github.com/apps/signpath) on `RamazanKara/Rubylight` only; SignPath uses it to read the repository's rulesets and audit log for pipeline policies.
10. **Create the artifact configurations.** In the project, add three artifact configurations with these slugs, choosing the custom XML option and pasting the file from the repository:
    - `binaries`: [.signpath/binaries.xml](../.signpath/binaries.xml)
    - `installer`: [.signpath/installer.xml](../.signpath/installer.xml)
    - `driver`: [.signpath/driver.xml](../.signpath/driver.xml). Only used once the display driver ships, but harmless to create now.

    SignPath's copy is the one that signs. When a file in `.signpath/` changes, paste it again.
11. **Create the CI user.** Under **Users**, add a CI user such as `rubylight-github`, generate its API token and keep it for step 13. It needs no other role.
12. **Configure the release signing policy** (usually slug `release-signing`, using the SignPath Foundation certificate):
    - **Submitters:** only the CI user from step 11.
    - **Approval process:** on, with you as the approver and one required approval. The Foundation requires a manual approval for every release.
    - **Trusted build system verification** and **Verify origin:** on.
    - **Allowed branch names:** `main` and the release tags. Releases come from a pushed tag (`refs/tags/v2.3.2`) or from `main` (**Run workflow** with **publish**). If SignPath turns the first request down on the branch check, its origin details show the branch name exactly as GitHub reported it; allow that form.
    - **Pipeline policy:** add one with this definition and set it to **Enforce**:

      ```yaml
      github-build-policies:
        version: '1.0'
        runners:
          require_github_hosted: true
      ```

      Every job already runs on GitHub-hosted runners. SignPath can also require the rulesets from step 3 with a `github-scm-policies` section ([reference](https://docs.signpath.io/trusted-build-systems/github#github-scm-policies)); try it on a release candidate first, because releases build from tags.

    Keep whatever stricter settings the Foundation has already made.

### In GitHub

13. **Add the secret.** Settings, Secrets and variables, Actions, **New repository secret**: `SIGNPATH_API_TOKEN` with the CI user's token.
14. **Add the variables**, on the **Variables** tab, in this order:
    1. `SIGNPATH_ORGANIZATION_ID`: the organization ID from SignPath's organization settings (a GUID).
    2. `SIGNPATH_SIGNING_POLICY_SLUG`: the release signing policy's slug, usually `release-signing`.
    3. `SIGNPATH_PROJECT_SLUG`: the project slug from step 8. Setting this one turns signing on for the next release run, so set it last.

### First signed release

15. **Release a candidate.** Bump to a release candidate (`python rust/release/bump.py 2.3.2-rc.1`), fill in its notes and push to `main`. Then use Actions, **Rubylight Windows**, **Run workflow** on `main` with **publish**. A failed signing run publishes nothing and creates no tag, so you can fix SignPath's settings and run it again.
16. **Approve the requests.** SignPath e-mails you for each request: first the binaries, then (once it ships) the driver catalog, then the installer. Approve each one that came from this run; its page shows the repository, commit and workflow run. Each request waits up to an hour.
17. **Check the result.** Download both release files and run `Get-AuthenticodeSignature` on the installer and on the executables and DLLs in the ZIP: each must be `Valid`, signed by SignPath Foundation, with a timestamp. Install, update and uninstall on a clean Windows PC, check the installed `uninstall.exe` the same way, and note what SmartScreen shows.
18. **Record it.** Name the first signed version in the [code signing policy](code-signing-policy.md), add the change to the release notes, and close [issue #25](https://github.com/RamazanKara/Rubylight/issues/25).

## Rubylight's display driver

[build.ps1](../rust/build.ps1) builds Rubylight's user-mode (UMDF IddCx) display driver from `rust/drivers/rubylight-vdisplay` into every package, so the driver steps of a signed release run by themselves:

- **Package layout.** `drivers/rubylight-display/` in the package holds one `.inf`, one `.dll` and their unsigned `.cat` (`CatalogFile=rubylight_vdisplay.cat`), plus `install-local.ps1` and `nefconc.exe`. `StageDriver` makes the catalog again from the signed DLL for `10_X64`.
- **Version information.** The DLL carries the same VERSIONINFO as every other binary: ProductName `Rubylight` and ProductVersion equal to the release version. The driver's build script writes it with [version_info.rs](../rust/assets/version_info.rs) and compiles it with the Windows Kit's `rc.exe`.
- **Signing order.** The `binaries` request signs the DLL. Inf2Cat then makes the catalog from the INF and the signed DLL, and the `driver` request signs the catalog after checking the DLL's signature and version information. This is the order Microsoft documents: signing the DLL after the catalog would change bytes the catalog has hashed. [sign-artifacts.ps1](../rust/release/sign-artifacts.ps1) checks that the signed catalog covers the INF and the DLL before the package is built.
- **SignPath.** Create the `driver` artifact configuration (step 10) before the first signed release, or that release fails at signing.
- **Tools.** The driver build and the signing steps take stampinf, Inf2Cat, signtool, rc.exe and the IddCx stub library from the Windows Driver Kit. The workflow installs the kit when the runner image lacks it.
- **Unsigned builds.** Until SignPath signs releases, and in every `main` and local build, the catalog is unsigned. Setup's `install-local.ps1` signs it on the user's PC with a key it deletes at once (see the driver's README).
- **Installing silently.** Windows asks before installing a driver from a publisher it doesn't trust yet. For a SignPath-signed catalog `install-local.ps1` adds that exact signer certificate to the machine's `TrustedPublisher` store before `pnputil`, as Vibepollo's installer does, and its `-Uninstall` removes the certificate again if it added it.

Kernel-mode drivers cannot be signed this way; Windows requires Microsoft's attestation or WHQL signature for them.

## Application text

The form at [signpath.org/apply](https://signpath.org/apply) asks for these fields.

| Field | Text |
| --- | --- |
| Project Name | Rubylight |
| Repository URL | https://github.com/RamazanKara/Rubylight |
| Homepage URL | https://ramazankara.github.io/Rubylight/ |
| Download URL | https://ramazankara.github.io/Rubylight/#install |
| Privacy Policy URL | https://ramazankara.github.io/Rubylight/docs/privacy.html |
| Wikipedia URL | (leave empty) |
| Tagline | A Windows game-streaming host for Moonlight clients, written in Rust and tuned for AMD Radeon GPUs. |
| Category | The closest to games or multimedia in the list. |
| Maintainer Type | Individual |
| Build System | GitHub Actions |
| First Name, Last Name | Ramazan Kara |
| Email | The address you want SignPath to use. |

**Description:**

> Rubylight streams games and the Windows desktop from a gaming PC to phones, tablets, TVs and other PCs running Moonlight, the open-source GameStream client. It is a GPL-3.0 host written in Rust that began as a fork of Vibepollo, from the Sunshine and Apollo family, and is built around the AMD Radeon frame path: screen capture, colour conversion and AMD's hardware encoder, with HDR, per-device virtual displays, controller and haptics support, and PyroWave for very low latency on local networks. It runs as a Windows service with a web console on the local PC. Releases are built from tagged commits by GitHub Actions on GitHub-hosted runners, with no manual build steps.
>
> We would like to sign because Windows warns about the unsigned installer: SmartScreen stops it with a "Windows protected your PC" warning and Smart App Control can block it outright, which pushes users to turn off protections they should keep. The installer also sets up a Windows service and drivers, so users should be able to see who published what they run. A future Rubylight virtual display driver (user-mode, IddCx) also needs a trusted signature on its catalog to install. Rubylight already ships the display and gamepad drivers your Foundation signed for Vibepollo, unchanged, with their original signatures.
>
> Rubylight has one maintainer, Ramazan Kara (GitHub: RamazanKara), who writes, reviews and releases the code and will approve every signing request. GitHub and SignPath access use multi-factor authentication. The code signing policy is at https://ramazankara.github.io/Rubylight/docs/code-signing-policy.html.

**Reputation** (update the numbers on the day you apply; the first three come from `gh api repos/RamazanKara/Rubylight/releases` and the issue list):

> Rubylight's Rust host was first published on GitHub at the end of September 2026, continuing the Butterpollo project, a fork of Vibepollo, whose drivers your Foundation signs. As of mid-October 2026 its 39 releases had been downloaded 3,030 times, 15 different people had reported issues on GitHub, and the repository had 58 stars. Users discuss it on Reddit: [add the links to your Reddit posts and the threads quoted in the README]. Rubylight has its own website with documentation (https://ramazankara.github.io/Rubylight/) and its own Android client, Nightjar (https://github.com/RamazanKara/nightjar). Since 2.3.0 every release must pass the full test suite and stream tests on real Radeon hardware before it ships, and attaches the results to its release page.

**Primary Discovery Channel:** your own answer. Vibepollo's SignPath Foundation signing is an honest "exact source".

## Names in the workflow

| Kind | Name | Value |
| --- | --- | --- |
| Secret | `SIGNPATH_API_TOKEN` | API token of the SignPath CI user (submitter) |
| Variable | `SIGNPATH_ORGANIZATION_ID` | SignPath organization ID |
| Variable | `SIGNPATH_SIGNING_POLICY_SLUG` | Release signing policy slug, usually `release-signing` |
| Variable | `SIGNPATH_PROJECT_SLUG` | SignPath project slug; setting it turns signing on |
| Artifact configuration | `binaries` | [.signpath/binaries.xml](../.signpath/binaries.xml) |
| Artifact configuration | `installer` | [.signpath/installer.xml](../.signpath/installer.xml) |
| Artifact configuration | `driver` | [.signpath/driver.xml](../.signpath/driver.xml) |

Sources: [SignPath Foundation conditions](https://signpath.org/terms), [SignPath's GitHub integration](https://docs.signpath.io/trusted-build-systems/github), [artifact configuration reference](https://docs.signpath.io/artifact-configuration/reference), [signing policies](https://docs.signpath.io/projects), [origin verification](https://docs.signpath.io/origin-verification).

Rubylight's own display driver (`rust/drivers/rubylight-vdisplay`) does not wait for
SignPath: until SignPath signs its catalog, it ships unsigned and
`install-local.ps1` signs it on each user's PC with a key it deletes at once. See the
driver's README, "Shipping before SignPath signs it".
