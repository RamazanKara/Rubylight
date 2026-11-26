"""Unit tests for .agents/hooks/guard_commands.py and watch_gpu_hang.py (Claude Code + Antigravity)."""
from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HOOKS_DIR = pathlib.Path(__file__).resolve().parent
if str(HOOKS_DIR) not in sys.path:
    sys.path.insert(0, str(HOOKS_DIR))

import guard_commands  # noqa: E402
import watch_gpu_hang  # noqa: E402


def _pre_payload(cmd: str, tool_name: str = "run_command", cwd: str | None = None) -> dict:
    p = {
        "conversationId": "test-conv",
        "hookEventName": "PreToolUse",
        "toolCall": {
            "id": "toolu_01",
            "name": tool_name,
            "args": {"CommandLine": cmd} if tool_name == "run_command" else {"Path": cmd},
        },
    }
    if cwd:
        p["cwd"] = cwd
    return p


def _claude_pre_payload(value: str, tool_name: str = "Bash", cwd: str | None = None) -> dict:
    if tool_name in ("Bash", "PowerShell"):
        tool_input = {"command": value}
    elif tool_name == "NotebookEdit":
        tool_input = {"notebook_path": value}
    else:
        tool_input = {"file_path": value}
    p: dict = {
        "session_id": "claude-test",
        "hook_event_name": "PreToolUse",
        "tool_name": tool_name,
        "tool_input": tool_input,
    }
    if cwd:
        p["cwd"] = cwd
    return p


class GuardCommandsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.agents_dir = pathlib.Path(self.tmp.name) / ".agents"
        self.agents_dir.mkdir(parents=True)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def _eval(self, cmd: str, env: dict[str, str] | None = None, cwd: str | None = None) -> dict[str, str]:
        return guard_commands.evaluate_command(
            cmd,
            agents_dir=self.agents_dir,
            env={} if env is None else env,
            cwd=cwd,
        )

    def test_safe_commands_allowed(self) -> None:
        safe = [
            "git status --short",
            "git push origin wip/roadmap/03-launch-curtain",
            "git push -u origin roadmap/redesign-and-waves-1-5",
            "cargo fmt -- --check",
            "cargo metadata --format-version 1",
            "tools\\dev.ps1 check -Fast -Base origin/main",
            r"pwsh C:\src\rl-quick.ps1 -Repo . -Crates butterpollo-session",
            "python tools/build_lock.py --lane cargo -- cargo test -p butterpollo-config --target-dir D:\\t\\cfg-test",
            r"python tools/build_lock.py --lane gradle --label nightjar -- cmd /c client\gradlew.bat -p client --no-daemon --max-workers=2 :app:testNonRootDebugUnitTest",
            "python tools/arch_check.py",
            "python .agents/tools/roadmap_hygiene.py status",
            "python .agents/tools/roadmap_hygiene.py pick-lane --no-ssh",
            "python .agents/tools/roadmap_hygiene.py clean --apply --remote-laptop",
            "python .agents/tools/roadmap_hygiene.py check-firewall --base roadmap/redesign-and-waves-1-5",
            "python tools/build_lock.py -- pwsh rust/build.ps1",
            "python tools\\build_lock.py --timeout 1800 -- pwsh rust\\build.ps1 -Package",
            "python -m unittest discover -s .agents -p test_*.py",
            "python -m http.server 8123 --bind 127.0.0.1",
            "npx vite preview --host 127.0.0.1 --port 4173",
            "python rust/release/test_gpu_hang.py",
            "gh workflow run release.yml -R RamazanKara/nightjar -f dry_run=true",
            "cat .claude/settings.json",
        ]
        for cmd in safe:
            with self.subTest(cmd=cmd):
                res = self._eval(cmd)
                self.assertEqual(res["decision"], "allow", msg=res)

    def test_always_hard_denied_including_bypass_vectors_hygiene_and_firewall(self) -> None:
        (self.agents_dir / guard_commands.RELEASE_SENTINEL).write_text("ok\n", encoding="utf-8", newline="\n")
        env = {}
        denied = [
            "git push --force origin wip/foo",
            "git.exe push -f origin roadmap/foo",
            "git push --force-with-lease origin wip/foo",
            "git push origin +HEAD:refs/heads/wip/foo",
            "git tag -f v7.0.0",
            "git.exe tag --force v7.0.0",
            "git tag -d v7.0.0",
            "git tag --delete v7.0.0",
            "git push origin :refs/tags/v7.0.0",
            "git push origin --delete v7.0.0",
            "cargo fmt --all -- --check",
            "cargo.exe +1.98.1-x86_64-pc-windows-gnu fmt --all",
            # Bare cargo / unlocked pool / bare gradlew (prevents private target/ garbage & daemon leaks)
            "cargo test -p butterpollo-config",
            "cargo clippy --workspace --all-targets",
            "cargo build --release",
            "python tools/build_lock.py --lane cargo -- cargo test -p butterpollo-config",
            "cargo test -p butterpollo-config --target-dir ./target",
            r"cargo test -p butterpollo-config --target-dir C:\src\rl-target-pool\1",
            r"client\gradlew.bat -p client :app:testNonRootDebugUnitTest",
            r"python tools/build_lock.py --lane gradle -- cmd /c client\gradlew.bat -p client assembleNonRootDebug",
            # Wildcard local dev servers (prevents Windows Defender Firewall popups)
            "python -m http.server 8000",
            "npx vite --host",
            "npm run dev -- --host 0.0.0.0",
            # Sensitive / guardrail / ratchet protection
            r"Get-Content C:\ProgramData\Butterpollo\config\logs\butterpollo.log",
            "cat /c/ProgramData/Butterpollo/config/butterpollo.conf",
            "type client/keystore.properties",
            "git add client/keystore.properties",
            "python tools/arch_check.py --write-baselines",
            r"pwsh rust\build.ps1",
            r"pwsh C:\src\rl-roadmap-prep\rust\build.ps1",
            "pwsh $PWD/rust/build.ps1",
            r"cd rust; .\build.ps1",
            "python tools/build_lock.py --lane cargo -- pwsh rust/build.ps1",
            "echo ok | pwsh rust/build.ps1",
            "$(pwsh rust/build.ps1)",
            "& { pwsh rust/build.ps1 }",
            "New-Item -ItemType File .agents/RELEASE_7_0_AUTHORIZED",
            "Remove-Item .agents/GPU_HANG_HALT",
            "Set-Content tools/size_ratchet.json '{}'",
            "Set-Content tools/arch_baselines.json '{}'",
            "Set-Content .agents/hooks.json '{}'",
            "rm .claude/settings.json",
            "echo '{}' > .claude/settings.local.json",
            "cat foo.json > .claude/settings.json",
            "sed -i 's/xhigh/low/' .claude/agents/wave-reviewer.md",
            "rm .agents/tools/roadmap_hygiene.py",
        ]
        for cmd in denied:
            with self.subTest(cmd=cmd):
                res = self._eval(cmd, env=env)
                self.assertEqual(res["decision"], "deny", msg=f"expected deny for {cmd}, got {res}")

    def test_file_tools_guarded_claude_code_and_antigravity(self) -> None:
        blocked_writes = [
            r"C:\src\rl-roadmap-prep\.agents\RELEASE_7_0_AUTHORIZED",
            r"C:\src\rl-roadmap-prep\.agents\GPU_HANG_HALT",
            r"C:\src\rl-roadmap-prep\.agents\hooks.json",
            r"C:\src\rl-roadmap-prep\.agents\hooks\guard_commands.py",
            r"C:\src\rl-roadmap-prep\.agents\tools\roadmap_hygiene.py",
            r"C:\src\rl-roadmap-prep\.claude\settings.json",
            r"C:\src\rl-roadmap-prep\.claude\settings.local.json",
            r"C:\src\rl-roadmap-prep\.claude\agents\wave-reviewer.md",
            r"C:\src\rl-roadmap-prep\tools\size_ratchet.json",
            r"C:\src\rl-roadmap-prep\tools\arch_baselines.json",
            r"C:\src\rl-roadmap-prep\client\keystore.properties",
            r"C:\ProgramData\Butterpollo\config\butterpollo.conf",
        ]
        for tool in ("write_file", "edit_file"):
            for path in blocked_writes:
                with self.subTest(client="antigravity", tool=tool, path=path):
                    res = guard_commands.handle_payload(_pre_payload(path, tool_name=tool), agents_dir=self.agents_dir)
                    self.assertEqual(res["decision"], "deny", msg=res)

        for tool in ("Write", "Edit", "MultiEdit", "NotebookEdit"):
            for path in blocked_writes:
                with self.subTest(client="claude", tool=tool, path=path):
                    res = guard_commands.handle_payload(_claude_pre_payload(path, tool_name=tool), agents_dir=self.agents_dir)
                    self.assertEqual(res["decision"], "deny", msg=res)

        for path in (r"C:\src\rl-roadmap-prep\tools\size_ratchet.json", r"C:\src\rl-roadmap-prep\.claude\settings.json"):
            self.assertEqual(
                guard_commands.handle_payload(_claude_pre_payload(path, tool_name="Read"), agents_dir=self.agents_dir)["decision"],
                "allow",
            )
        for secret in (r"C:\ProgramData\Butterpollo\config\logs\butterpollo.log", r"C:\src\rl-roadmap-prep\client\keystore.properties"):
            self.assertEqual(
                guard_commands.handle_payload(_claude_pre_payload(secret, tool_name="Read"), agents_dir=self.agents_dir)["decision"],
                "deny",
            )
            self.assertEqual(
                guard_commands.handle_payload(_pre_payload(secret, tool_name="read_file"), agents_dir=self.agents_dir)["decision"],
                "deny",
            )

    def test_nested_claude_worktree_inherits_enclosing_repo_sentinels(self) -> None:
        repo = pathlib.Path(self.tmp.name) / "repo"
        (repo / ".git").mkdir(parents=True)
        repo_agents = repo / ".agents"
        repo_agents.mkdir(parents=True)
        wt = repo / ".claude" / "worktrees" / "w1-07"
        wt_agents = wt / ".agents"
        wt_agents.mkdir(parents=True)
        (wt / ".git").write_text("gitdir: ../../.git/worktrees/w1-07\n", encoding="utf-8", newline="\n")

        (repo_agents / guard_commands.GPU_HANG_SENTINEL).write_text("halted\n", encoding="utf-8", newline="\n")
        res = guard_commands.evaluate_command(
            "python rust/release/e2e.py --codec hevc",
            agents_dir=wt_agents,
            env={},
            cwd=str(wt),
        )
        self.assertEqual(res["decision"], "deny", msg=res)

    def test_programdata_butterpollo_release_not_false_positive(self) -> None:
        res = self._eval(r"Test-Path C:\ProgramData\ButterpolloRelease\elevated.ps1")
        self.assertEqual(res["decision"], "allow", msg=res)

    def test_release_commands_require_sentinel_and_no_gpu_halt(self) -> None:
        release_cmds = [
            "git push origin main",
            "git push",
            "git push origin HEAD",
            "git push origin HEAD:refs/heads/main",
            "git push origin abc123:refs/tags/v7.0.0",
            "git push origin --tags",
            "git push origin v7.0.0",
            "gh release create v7.0.0",
            "gh.exe release edit v7.0.0 --notes-file notes.md",
            "gh api repos/RamazanKara/Rubylight/releases -X POST",
            "gh workflow run rust-windows.yml -f publish=true",
            "gh workflow run release.yml -R RamazanKara/nightjar -f dry_run=false",
            "git push origin gh-pages",
            "git -C C:\\temp\\pages push -u origin gh-pages",
        ]
        for cmd in release_cmds:
            with self.subTest(phase="no_sentinel", cmd=cmd):
                self.assertEqual(self._eval(cmd)["decision"], "deny")

        (self.agents_dir / guard_commands.RELEASE_SENTINEL).write_text("authorized\n", encoding="utf-8", newline="\n")
        for cmd in release_cmds:
            with self.subTest(phase="authorized", cmd=cmd):
                res = self._eval(cmd)
                self.assertEqual(res["decision"], "allow", msg=res)

        (self.agents_dir / guard_commands.GPU_HANG_SENTINEL).write_text("halted\n", encoding="utf-8", newline="\n")
        for cmd in release_cmds:
            with self.subTest(phase="gpu_halted", cmd=cmd):
                self.assertEqual(self._eval(cmd)["decision"], "deny")

    def test_hw_commands_allowed_by_default_and_blocked_by_disable_or_gpu_halt(self) -> None:
        hw_cmds = [
            "python rust/release/e2e.py --codec hevc",
            r"pwsh tools\hw\run.ps1 -Batch desktop",
            "python tools/hw/run.py --batch desktop",
            r"pwsh tools\hw\laptop-host.ps1 -Out C:\temp\out",
            r"pwsh rust\release\check.ps1 v7.0.0",
            r"pwsh rust\release\soak.ps1",
            "python rust/release/soak.py",
            "python rust/tests/crash_restore_soak.py",
            "python rust/release/focus_ab.py quick",
            "python tools/hw/wave1_verify.py",
            "python tools/build_lock.py --lane cargo -- cargo test -p butterpollo-windows --target-dir D:\\t\\hw-ign -- --ignored",
            "python tools/build_lock.py --lane cargo -- cargo nextest run --target-dir D:\\t\\hw-ign --run-ignored only",
        ]
        for cmd in hw_cmds:
            with self.subTest(phase="default_allow", cmd=cmd):
                self.assertEqual(self._eval(cmd, env={})["decision"], "allow")

        for cmd in hw_cmds:
            with self.subTest(phase="explicit_disable", cmd=cmd):
                self.assertEqual(self._eval(cmd, env={guard_commands.HW_DISABLE_ENV: "1"})["decision"], "deny")

        (self.agents_dir / guard_commands.GPU_HANG_SENTINEL).write_text("halted\n", encoding="utf-8", newline="\n")
        for cmd in hw_cmds:
            with self.subTest(phase="gpu_halted", cmd=cmd):
                self.assertEqual(self._eval(cmd, env={})["decision"], "deny")

        for remote_cmd in (
            r"pwsh tools\remote-check.ps1 -Host laptop -Branch wip/roadmap/03-launch-curtain",
            "git push -u origin wip/roadmap/03-launch-curtain",
        ):
            with self.subTest(phase="remote_lane", cmd=remote_cmd):
                self.assertEqual(self._eval(remote_cmd, env={})["decision"], "allow")

    def test_subprocess_cli_contracts_claude_code_and_antigravity(self) -> None:
        env = dict(os.environ, RUBYLIGHT_AGENTS_DIR=str(self.agents_dir))
        env.pop(guard_commands.HW_DISABLE_ENV, None)

        proc_cc_deny = subprocess.run(
            [sys.executable, str(HOOKS_DIR / "guard_commands.py")],
            input=json.dumps(_claude_pre_payload("git push origin main", tool_name="Bash")),
            capture_output=True,
            text=True,
            env=env,
            check=True,
        )
        out_cc_deny = json.loads(proc_cc_deny.stdout)
        hso = out_cc_deny["hookSpecificOutput"]
        self.assertEqual(hso["hookEventName"], "PreToolUse")
        self.assertEqual(hso["permissionDecision"], "deny")
        self.assertIn("RELEASE_7_0_AUTHORIZED", hso["permissionDecisionReason"])

        proc_cc_allow = subprocess.run(
            [sys.executable, str(HOOKS_DIR / "guard_commands.py")],
            input=json.dumps(_claude_pre_payload("git status --short", tool_name="PowerShell")),
            capture_output=True,
            text=True,
            env=env,
            check=True,
        )
        self.assertEqual(json.loads(proc_cc_allow.stdout), {})

        proc_ag = subprocess.run(
            [sys.executable, str(HOOKS_DIR / "guard_commands.py")],
            input=json.dumps(_pre_payload("git push origin main")),
            capture_output=True,
            text=True,
            env=env,
            check=True,
        )
        out_ag = json.loads(proc_ag.stdout)
        self.assertEqual(out_ag["decision"], "deny")
        self.assertIn("RELEASE_7_0_AUTHORIZED", out_ag["reason"])

        proc_bad = subprocess.run(
            [sys.executable, str(HOOKS_DIR / "guard_commands.py")],
            input="{not-json",
            capture_output=True,
            text=True,
            env=env,
            check=True,
        )
        out_bad = json.loads(proc_bad.stdout)
        self.assertEqual(out_bad["decision"], "deny")
        self.assertEqual(out_bad["hookSpecificOutput"]["permissionDecision"], "deny")


class WatchGpuHangTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)
        self.agents_dir = self.root / "main_repo" / ".agents"
        self.agents_dir.mkdir(parents=True)
        self.wt_agents_dir = self.root / "share_wt" / ".agents"
        self.wt_agents_dir.mkdir(parents=True)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_claude_code_post_tool_use_and_nested_worktree_propagation(self) -> None:
        repo = self.root / "cc_repo"
        (repo / ".git").mkdir(parents=True)
        repo_agents = repo / ".agents"
        repo_agents.mkdir(parents=True)
        wt = repo / ".claude" / "worktrees" / "w1-07"
        wt_agents = wt / ".agents"
        wt_agents.mkdir(parents=True)
        (wt / ".git").write_text("gitdir: ../../.git/worktrees/w1-07\n", encoding="utf-8", newline="\n")

        payload = {
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_use_id": "toolu_cc_3",
            "cwd": str(wt),
            "tool_input": {"command": "python rust/release/e2e.py --codec hevc"},
            "tool_response": {"stdout": "", "stderr": "Exit code 3\nencoder hang", "exitCode": 3},
        }
        triggered = watch_gpu_hang.process_post_tool(payload, agents_dir=wt_agents)
        self.assertTrue(triggered)
        for target in (wt_agents, repo_agents):
            halt = target / watch_gpu_hang.GPU_HANG_SENTINEL
            self.assertTrue(halt.is_file(), msg=f"missing halt in {target}")
            self.assertNotIn(b"\r\n", halt.read_bytes())

    def test_claude_code_transcript_fallback(self) -> None:
        transcript = self.root / "cc_transcript.jsonl"
        transcript.write_text(
            json.dumps({
                "type": "tool_result",
                "tool_use_id": "toolu_cc_99",
                "content": "Running soak...\nExit code 3\n",
            }) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        payload = {
            "hook_event_name": "PostToolUse",
            "tool_name": "PowerShell",
            "tool_use_id": "toolu_cc_99",
            "transcript_path": str(transcript),
            "tool_input": {"command": "python rust/tests/crash_restore_soak.py"},
            "tool_response": {"stdout": "done"},
        }
        self.assertTrue(watch_gpu_hang.process_post_tool(payload, agents_dir=self.agents_dir))

    def test_writes_sentinel_on_antigravity_transcript_fallback_and_multi_workspace(self) -> None:
        transcript = self.root / "transcript.jsonl"
        transcript.write_text(
            json.dumps({
                "step_index": 12,
                "source": "MODEL",
                "type": "GENERIC",
                "status": "DONE",
                "content": "Running soak...\nCommand exited with code 3.\n",
            }) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        payload = {
            "hookEventName": "PostToolUse",
            "stepIdx": 12,
            "transcriptPath": str(transcript),
            "workspacePaths": [str(self.root / "main_repo")],
            "cwd": str(self.root / "share_wt"),
            "toolCall": {
                "id": "toolu_12",
                "name": "run_command",
                "args": {"CommandLine": "python rust/tests/crash_restore_soak.py"},
            },
        }
        triggered = watch_gpu_hang.process_post_tool(payload, agents_dir=self.agents_dir)
        self.assertTrue(triggered)
        for target in (self.agents_dir, self.wt_agents_dir):
            halt = target / watch_gpu_hang.GPU_HANG_SENTINEL
            self.assertTrue(halt.is_file(), msg=f"missing halt in {target}")
            self.assertNotIn(b"\r\n", halt.read_bytes())

    def test_watched_runners_trigger_and_non_exit_3_ignored(self) -> None:
        for cmd in (
            "python rust/release/e2e.py --codec hevc",
            r"pwsh rust\release\check.ps1 v7.0.0",
            r"pwsh tools\hw\run.ps1 -Batch desktop",
            "python rust/release/soak.py",
            "python rust/release/focus_ab.py quick",
        ):
            payload = {
                "hook_event_name": "PostToolUse",
                "tool_name": "Bash",
                "tool_input": {"command": cmd},
                "tool_response": {"exit_code": 3},
            }
            self.assertTrue(watch_gpu_hang.process_post_tool(payload, agents_dir=self.agents_dir), msg=cmd)

        other_exit = {
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "python rust/release/e2e.py"},
            "tool_response": {"exitCode": 1, "stderr": "exit status 1"},
        }
        self.assertFalse(watch_gpu_hang.process_post_tool(other_exit, agents_dir=self.agents_dir))

        remote_timeout = {
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": r"pwsh tools\remote-check.ps1 -Host laptop -Branch wip/roadmap/03-launch-curtain"},
            "tool_response": {"exitCode": 3},
        }
        self.assertFalse(watch_gpu_hang.process_post_tool(remote_timeout, agents_dir=self.agents_dir))


    def test_reading_or_naming_a_runner_is_not_a_run(self) -> None:
        for cmd in (
            "sed -n 5,14p tools/hw/run.py; sed -n 326,337p tools/hw/run.py",
            "grep -n soak tools/hw/run.py",
            "cat > notes.md <<'EOF'\nrun python rust/tests/crash_restore_soak.py; Exit code 3 is an AMD hang\nEOF",
            "python - <<'EOF'\nprint('rust/release/e2e.py exit code 3')\nEOF",
            "git log --oneline -- rust/release/e2e.py",
        ):
            payload = {
                "hook_event_name": "PostToolUse",
                "tool_name": "Bash",
                "tool_input": {"command": cmd},
                "tool_response": {"stdout": "Exit code 3 is an AMD encoder hang", "exit_code": 3},
            }
            self.assertFalse(watch_gpu_hang.process_post_tool(payload, agents_dir=self.agents_dir), msg=cmd)
        for cmd in (
            "MSYS_NO_PATHCONV=1 python rust/release/e2e.py --codec av1",
            "cd rust && python tests/crash_restore_soak.py --cycles 10",
            "python C:/Users/ramaz/.claude/tools/laptop.py run --branch x -- python rust/release/e2e.py",
        ):
            payload = {
                "hook_event_name": "PostToolUse",
                "tool_name": "Bash",
                "tool_input": {"command": cmd},
                "tool_response": {"exit_code": 3},
            }
            self.assertTrue(watch_gpu_hang.process_post_tool(payload, agents_dir=self.agents_dir), msg=cmd)

if __name__ == "__main__":
    unittest.main()
