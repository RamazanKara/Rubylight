# Release-note fragments (host)

Don't edit `rust/RELEASE_NOTES.md` on a topic branch. Write `rust/release/notes.d/<topic>.md` instead, one file per topic, so parallel branches never conflict on the notes.

A fragment is a list of user-facing bullets in the style of `RELEASE_NOTES.md`: what changes for the user, measured numbers only, "Experimental" for what our devices can't prove. An optional `# Topic` title line and HTML comments are dropped; anything else that isn't a `- ` bullet or an indented continuation line fails the assembly.

```markdown
# Encoder focus
- **Sharper text where you look.** ... Measured on the RX 7900 XT: ...
- Experimental: ...
```

At release time `python rust/release/bump.py X.Y.Z` moves every fragment, in file name order, under `## Unreleased` and from there into `## New in X.Y.Z`, then deletes the fragments. `python rust/release/fragments.py` does the first step alone (to collect them between releases). This README stays.
