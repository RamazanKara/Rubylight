# Windows release signing

The Windows workflow supports SignPath for Rubylight's executables and DLLs,
including the installer and uninstaller. Signing needs an approved project and a
production certificate in SignPath; repository code cannot provision either.
Until that account configuration is supplied, releases remain unsigned.

The repository administrator enables signing by configuring:

- Secret `SIGNPATH_API_TOKEN` for the CI submitter account.
- Variables `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG` and
  `SIGNPATH_SIGNING_POLICY_SLUG` for the approved production signing policy.
- Artifact configurations named `binaries` and `installer`, using
  [binaries.xml](../.signpath/binaries.xml) and
  [installer.xml](../.signpath/installer.xml).
- A trusted GitHub build integration restricted to this repository's Windows
  workflow, protected `main` and release tags. Untrusted pull requests never
  receive the signing token.

Follow [SignPath's GitHub integration instructions](https://docs.signpath.io/trusted-build-systems/github)
when granting the build integration access and configuring policy approval.
Configure the token and organization/policy variables first, then set the project
variable last to enable signing. A configured signing request that fails, lacks a
file, or returns an invalid or untimestamped signature fails the build; it never
silently publishes unsigned binaries.

The workflow signs Rubylight's own binaries first, regenerates the SHA-256 package
manifest, builds the ZIP and self-extracting installer, then signs the complete
installer. Third-party drivers and DLLs retain their existing upstream identity.
The separate signed setup executable inside the package becomes `uninstall.exe`;
removing an outer installer's appended payload would invalidate its signature.
The payload reader recognizes the PE certificate table and alignment padding so
the appended installer signature does not hide the package footer.

After enabling the account configuration, verify both downloaded release assets:
`Get-AuthenticodeSignature` must report `Valid` with the expected publisher and a
timestamp for all Rubylight executables/DLLs. Install, update and uninstall on a
clean Windows machine, and verify the installed `uninstall.exe` as well.
