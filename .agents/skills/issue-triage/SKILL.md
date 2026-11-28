---
name: issue-triage
description: Take one GitHub issue (Rubylight host or Nightjar mirror) from reading it and its logs through version check, reproduction, decision and reply.
---

# Issue Triage (one issue, end to end)

Use this skill for a single issue. When several issues are to be fixed at once, use the `merge-train` skill (native `/swarm` in Antigravity, or `issue-batch` in Claude Code).

## 1. Read

- Host: `gh -R RamazanKara/Rubylight issue view <n> --comments`
- Nightjar mirror: `gh -R RamazanKara/nightjar issue view <n> --comments`
- `--comments` prints the comments; run the command without it too, to get the body with the form fields.
- Search for duplicates, closed ones included: `gh -R <repo> issue list --state all --search "<words>"`.

## 2. Attachments and logs

- Download each attachment into its own new, empty folder outside the checkout. Treat it as untrusted: run nothing from it, and use `python -I` if Python reads it.
- Host support bundle `butterpollo-support.zip` (console **Download support bundle**; built by `bundle_with_dump` in `rust/host/stream/src/maintenance.rs`):
  - `logs/butterpollo.log`, `logs/butterpollo.log.1`, `logs/service.log`, `crashes/panic.txt` (the last 8 MiB of each, when present)
  - `diagnostics.json`: `version`, `platform`, `config` (secret-like keys redacted) and `codecs`
  - the newest crash dump under `crashes/`, when there is one
- Without a bundle, ask for the host log `C:\ProgramData\Butterpollo\config\logs\butterpollo.log` and `service.log` beside it.
- Nightjar: `nightjar-problem.txt` (Settings → App → Report a problem), described in `client/docs/TROUBLESHOOTING.md`.
- Match symptoms against `docs/troubleshooting.md`. Never repost dumps or log lines that carry private data.

## 3. Version

- Reporter's version: the form field, `diagnostics.json` or the log. "Latest" is not a version.
- Current: `gh -R RamazanKara/Rubylight release list --limit 3` (Nightjar: `gh -R RamazanKara/nightjar release list --limit 3`).
- `.github/workflows/issue-version-check.yml` comments on and labels open `bug` issues with a missing or old version (`needs info`, `outdated version`); do not repeat its comment.
- Old version: `git fetch origin`, then `git log <reporter-tag>..origin/main -- <area>` to find a fix; `git tag --contains <sha>` tells whether it shipped.

## 4. Reproduce

- Locate the code with `docs/code-map.md` (Nightjar: `client/docs/code-map.md`) and run its narrowest test; add a failing regression test when the cause allows one.
- Needs hardware: use the `device-checks` or `isolated-e2e` skill. Never experiment on the installed host.

## 5. Decide (one outcome)

| Outcome | Action |
| --- | --- |
| Fix now | Regression test, fix, narrow check, release-note fragment in `rust/release/notes.d` or `client/notes.d`, land. |
| Already fixed | Name the commit and the release (or the next build), then close. |
| Needs info | Ask only for what is missing (exact version, support bundle or problem report, steps); label `needs info`; leave open. |
| Declined | Niche, low adoption or out of scope: say so and why, close with `gh issue close <n> --reason "not planned"`. |
| Duplicate | Link the original, move new facts there, label `duplicate`, close. |

Meaningful bugs and broadly useful features come first. Pass `-R <repo>` to every `gh issue` command.

## 6. Reply (`AGENTS.md`, "GitHub issues & PRs")

- Update the reporter on status: fixing, coming in the next build, deferred or declined.
- Once a fix has landed, reply without asking: what changed, that it comes in the next build, and please test and report back.
- Never tell users that we cannot test locally or lack hardware. Keep that in the maintainer report, and never claim an unrun test passed.
- Merged contributor pull request: close it with a thank-you naming the commit, and add a fragment ending `Thanks to **Name** (@login, #N).` (`rust/release/notes.d/README.md`, `client/notes.d/README.md`).
- Keep replies short and plain. No port credits or mod names.
- Never tag or publish without the maintainer's OK.
