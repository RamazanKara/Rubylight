// Checks the version fields of a bug report against the latest releases and
// leaves one comment (updated in place) plus labels when something is off.
// Runs from .github/workflows/issue-version-check.yml; config comes from env:
//   APP_NAME, APP_HEADING     product and the form heading of its version field
//   HOST_REPO, HOST_NAME, HOST_HEADING   optional second product to check
//   DRY_RUN                              print writes instead of sending them
import { readFileSync } from "node:fs";

const env = process.env;
const repo = env.GITHUB_REPOSITORY;
const { issue } = JSON.parse(readFileSync(env.GITHUB_EVENT_PATH, "utf8"));
const MARKER = "<!-- issue-version-check -->";
const NEEDS_INFO = "needs info";
const OUTDATED = "outdated version";

async function api(path, init = {}) {
  if (env.DRY_RUN && init.method) return console.log(init.method, path, init.body ?? "");
  const res = await fetch(`https://api.github.com${path}`, {
    ...init,
    headers: {
      authorization: `Bearer ${env.GITHUB_TOKEN}`,
      accept: "application/vnd.github+json",
      "content-type": "application/json",
    },
  });
  if (res.status === 404 && init.method === "DELETE") return null;
  if (!res.ok) throw new Error(`${init.method ?? "GET"} ${path}: ${res.status} ${await res.text()}`);
  return res.status === 204 ? null : res.json();
}

function section(body, heading) {
  const m = body.match(new RegExp(`^### ${heading}\\s*\\n([\\s\\S]*?)(?=^### |(?![\\s\\S]))`, "m"));
  return m ? m[1].trim() : "";
}

function parse(text) {
  const m = text.match(/(\d+)\.(\d+)(?:\.(\d+))?(?:-([0-9A-Za-z.]+))?/);
  return m ? { nums: [+m[1], +m[2], +(m[3] ?? 0)], pre: m[4] ?? null, text: m[0] } : null;
}

function older(a, b) {
  for (let i = 0; i < 3; i++) if (a.nums[i] !== b.nums[i]) return a.nums[i] < b.nums[i];
  return a.pre !== null && b.pre === null;
}

async function latest(r) {
  const rel = await api(`/repos/${r}/releases/latest`);
  return { ...parse(rel.tag_name), url: rel.html_url };
}

if (!issue.labels.some((l) => l.name === "bug") || issue.state !== "open") process.exit(0);

const body = issue.body ?? "";
const notes = [];
let needsInfo = false;
let outdated = false;

const appLatest = await latest(repo);
const app = parse(section(body, env.APP_HEADING));
if (!app) {
  needsInfo = true;
  notes.push(
    `**${env.APP_NAME} version** has no version number. "Latest" changes every few days, so please edit the issue and put the exact number (the current release is [${appLatest.text}](${appLatest.url})).`,
  );
} else if (older(app, appLatest)) {
  outdated = true;
  notes.push(
    `You're on ${env.APP_NAME} **${app.text}**; the latest release is [**${appLatest.text}**](${appLatest.url}). Please update and check whether this still happens, then edit the version in the issue.`,
  );
}

if (env.HOST_REPO) {
  const hostText = section(body, env.HOST_HEADING);
  if (new RegExp(env.HOST_NAME, "i").test(hostText)) {
    const hostLatest = await latest(env.HOST_REPO);
    const host = parse(hostText);
    if (!host) {
      needsInfo = true;
      notes.push(`Please add the exact ${env.HOST_NAME} version from its console (current: [${hostLatest.text}](${hostLatest.url})).`);
    } else if (older(host, hostLatest)) {
      outdated = true;
      notes.push(
        `The host is ${env.HOST_NAME} **${host.text}**; the latest is [**${hostLatest.text}**](${hostLatest.url}). Many client problems are fixed on the host side, so please update it too.`,
      );
    }
  }
}

const n = issue.number;
const comments = await api(`/repos/${repo}/issues/${n}/comments?per_page=100`);
const mine = comments.find((c) => c.body.includes(MARKER));
// Only take back labels this script added, so a maintainer's own "needs info" survives edits.
const added = new Set(mine ? (mine.body.match(/<!-- labels: (.*?) -->/)?.[1] ?? "").split(",") : []);

if (notes.length) {
  const labels = [needsInfo && NEEDS_INFO, outdated && OUTDATED].filter(Boolean).join(",");
  const text = `${MARKER}<!-- labels: ${labels} -->\n${notes.map((t) => `- ${t}`).join("\n")}\n\nThis comment updates itself when you edit the issue.`;
  if (!mine) await api(`/repos/${repo}/issues/${n}/comments`, { method: "POST", body: JSON.stringify({ body: text }) });
  else if (mine.body !== text) await api(`/repos/${repo}/issues/comments/${mine.id}`, { method: "PATCH", body: JSON.stringify({ body: text }) });
} else if (mine) {
  await api(`/repos/${repo}/issues/comments/${mine.id}`, { method: "DELETE" });
}

const has = new Set(issue.labels.map((l) => l.name));
for (const [label, want] of [[NEEDS_INFO, needsInfo], [OUTDATED, outdated]]) {
  if (want && !has.has(label)) {
    await api(`/repos/${repo}/issues/${n}/labels`, { method: "POST", body: JSON.stringify({ labels: [label] }) });
  } else if (!want && has.has(label) && added.has(label)) {
    await api(`/repos/${repo}/issues/${n}/labels/${encodeURIComponent(label)}`, { method: "DELETE" });
  }
}
console.log(`#${n}: needsInfo=${needsInfo} outdated=${outdated}`);
