# Setup with a leftover settings folder (#61)
- **Setup no longer stops on a leftover `C:\ProgramData\Butterpollo` that your own account owns.** With no Rubylight installed, setup can't tell who made that folder, so it doesn't use the settings or pairings in it. It moves the folder to `Butterpollo.untrusted-<time>` unchanged, starts with new settings and says where the old folder went.
