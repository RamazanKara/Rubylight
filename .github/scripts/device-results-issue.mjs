// Turns an open "Device results" issue into a row in the device table: validates the pasted
// results, updates site/data/device-results.json and docs/devices.md on main, copies the data to
// gh-pages, then comments on the issue and closes it. Failures are explained in one comment that
// is edited in place; editing the issue runs the workflow again.
// Runs from .github/workflows/device-results.yml with GITHUB_EVENT_PATH, GITHUB_REPOSITORY and
// GITHUB_TOKEN. The issue body is only ever read as data from the event file, never from a shell.
//
//   node .github/scripts/device-results-issue.mjs [--event event.json] [--dry-run]
//       [--data site/data/device-results.json] [--docs docs/devices.md] [--today YYYY-MM-DD]
//
// --dry-run (or DRY_RUN=1) computes everything and prints what would be written, committed,
// commented and closed, without touching the files, git or the API.
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseResults, replaceEntry, textProblems, validateResults } from '../../scripts/add-device-results.mjs';
import { applyToDocs } from '../../scripts/device-docs.mjs';

export const MARKER = '<!-- device-results-bot -->';
export const LABEL = 'device-results';
export const SITE_URL = 'https://ramazankara.github.io/nightjar/devices.html';
export const DOCS_URL = 'https://github.com/RamazanKara/nightjar/blob/main/docs/devices.md#devices';
const BOT = 'github-actions[bot]';
const BOT_EMAIL = '41898282+github-actions[bot]@users.noreply.github.com';
const MAX_PROBLEMS = 12;

/** The text under "### heading" in an issue form body, trimmed; empty when the section is missing. */
export function section(body, heading) {
  const escaped = heading.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = body.replace(/\r\n?/g, '\n').match(new RegExp('^### ' + escaped + '[ \\t]*\\n([\\s\\S]*?)(?=^### |(?![\\s\\S]))', 'm'));
  return match ? match[1].trim() : '';
}

const answered = text => (text === '_No response_' ? '' : text);

/** Splits a device results issue form into its answers. */
export function parseIssue(body) {
  const text = body ?? '';
  return {
    name: answered(section(text, 'Device')).split('\n')[0].trim(),
    resultsText: answered(section(text, 'Results text')),
    stream: answered(section(text, 'Stream on this device (optional)')),
    consent: /^\s*-\s*\[[xX]\]/m.test(section(text, 'Publishing'))
  };
}

/** Whether this event is about an open device results issue (not a pull request). */
export function isDeviceResultsIssue(issue) {
  return Boolean(issue) && !issue.pull_request && issue.state === 'open' && (issue.labels ?? []).some(label => label.name === LABEL);
}

const safe = text => String(text).replace(/@/g, '@​').replace(/`/g, "'");

/**
 * Decides what to do with an issue. `table` is changed in place when the result is 'ok' and the
 * action is 'added' or 'updated'. Returns { status: 'invalid', problems } or
 * { status: 'ok', action, hold, name, dropped }; `hold` is true when the issue must stay open.
 */
export function evaluate({ issue, body, table, today }) {
  const parsed = parseIssue(body);
  const problems = [];
  if (!parsed.consent) problems.push('The "Publishing" box is not ticked. Tick it to agree that the results are published, or close this issue if you do not want that.');
  if (!parsed.name) problems.push('The "Device" field is empty. Enter the device name as it should appear in the table.');
  else for (const problem of textProblems(parsed.name)) problems.push('Device name: ' + problem + '.');

  let results = null;
  let dropped = [];
  if (!parsed.resultsText) {
    problems.push('The "Results text" field is empty. Paste the whole text from Share results in Nightjar.');
  } else {
    try {
      const checked = validateResults(parseResults(parsed.resultsText));
      dropped = checked.dropped;
      results = checked.results;
      for (const error of checked.errors) problems.push('Results text, ' + error);
    } catch (error) {
      problems.push('The results text is not valid JSON (' + error.message + '). Paste the whole text from Share results, from the first { to the last }.');
    }
  }
  if (problems.length) return { status: 'invalid', problems: problems.map(safe) };

  let action;
  try {
    action = replaceEntry(table, results, { issue, name: parsed.name, added: today });
  } catch (error) {
    return { status: 'invalid', problems: [safe(error.message)] };
  }
  return { status: 'ok', action, hold: parsed.stream !== '', name: parsed.name, dropped };
}

export function successComment({ action, hold, dropped }) {
  const lines = [MARKER];
  lines.push(
    (action === 'updated' ? 'Thanks, I updated your results in the table.' : 'Thanks, your results are in the table.') +
      ' You can see them in the [device results](' + SITE_URL + ') (the site refreshes within a couple of minutes) and in the [summary table](' + DOCS_URL + ').'
  );
  if (dropped.length) lines.push('', 'These fields are not part of the results format and were left out: ' + dropped.slice(0, 6).map(field => '`' + safe(field) + '`').join(', ') + '.');
  lines.push('');
  if (hold) {
    lines.push('I am leaving this issue open because "Stream on this device" has details that someone should read first.');
  } else {
    lines.push('Open a new "Device results" issue for another device or for new results. If there is more to report about this one, reply here and it will be reopened.');
  }
  return lines.join('\n');
}

export function failureComment(problems) {
  const shown = problems.slice(0, MAX_PROBLEMS);
  const more = problems.length - shown.length;
  return [
    MARKER,
    'Thanks for the results. I could not add them to the table yet:',
    '',
    ...shown.map(problem => '- ' + problem),
    ...(more > 0 ? ['- and ' + more + ' more'] : []),
    '',
    'Please edit this issue to fix it. The check runs again when you save. The steps are in the [device results guide](https://github.com/RamazanKara/nightjar/blob/main/docs/devices.md).'
  ].join('\n');
}

function options(args) {
  const parsed = {
    event: process.env.GITHUB_EVENT_PATH,
    data: 'site/data/device-results.json',
    docs: 'docs/devices.md',
    today: new Date().toISOString().slice(0, 10),
    dryRun: Boolean(process.env.DRY_RUN)
  };
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === '--event') parsed.event = args[++i];
    else if (arg === '--data') parsed.data = args[++i];
    else if (arg === '--docs') parsed.docs = args[++i];
    else if (arg === '--today') parsed.today = args[++i];
    else if (arg === '--dry-run') parsed.dryRun = true;
    else throw new Error('unexpected argument ' + arg);
  }
  if (!parsed.event) throw new Error('no event file: set GITHUB_EVENT_PATH or pass --event');
  return parsed;
}

const git = (args, cwd) => execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] });

function pushWithRetry(prepare, push) {
  for (let attempt = 1; ; attempt++) {
    try {
      prepare();
      return push();
    } catch (error) {
      if (attempt >= 3) throw error;
      console.warn('Push failed (' + attempt + '/3), retrying from the new tip: ' + error.message.split('\n')[0]);
    }
  }
}

/** Copies the data file to data/ on gh-pages and pushes when it differs. */
function publishToPages(dataFile) {
  const dir = join(mkdtempSync(join(process.env.RUNNER_TEMP ?? tmpdir(), 'gh-pages-')), 'site');
  git(['fetch', '--depth=1', 'origin', 'gh-pages']);
  git(['worktree', 'add', '--detach', dir, 'FETCH_HEAD']);
  let changed = false;
  pushWithRetry(
    () => {
      git(['fetch', '--depth=1', 'origin', 'gh-pages'], dir);
      git(['reset', '--hard', 'FETCH_HEAD'], dir);
      mkdirSync(join(dir, 'data'), { recursive: true });
      copyFileSync(dataFile, join(dir, 'data', 'device-results.json'));
      changed = Boolean(git(['status', '--porcelain', '--', 'data/device-results.json'], dir).trim());
      if (!changed) return;
      git(['add', 'data/device-results.json'], dir);
      git(['commit', '-m', 'docs(site): publish device results\n\nSigned-off-by: ' + BOT + ' <' + BOT_EMAIL + '>'], dir);
    },
    () => {
      if (changed) git(['push', 'origin', 'HEAD:gh-pages'], dir);
    }
  );
}

export async function main(args) {
  const opts = options(args);
  const env = process.env;
  const repo = env.GITHUB_REPOSITORY ?? 'RamazanKara/nightjar';
  const { issue } = JSON.parse(readFileSync(opts.event, 'utf8'));
  if (!isDeviceResultsIssue(issue)) return console.log('Not an open device results issue; nothing to do.');
  if (!Number.isInteger(issue.number) || issue.number <= 0) throw new Error('the event has no issue number');
  const n = issue.number;
  const dataFile = resolve(opts.data);
  const docsFile = resolve(opts.docs);

  async function api(path, init = {}) {
    const res = await fetch((env.GITHUB_API_URL ?? 'https://api.github.com') + path, {
      ...init,
      headers: { authorization: 'Bearer ' + env.GITHUB_TOKEN, accept: 'application/vnd.github+json', 'content-type': 'application/json' }
    });
    if (!res.ok) throw new Error((init.method ?? 'GET') + ' ' + path + ': ' + res.status + ' ' + (await res.text()));
    return res.status === 204 ? null : res.json();
  }

  async function comment(text) {
    if (opts.dryRun) return console.log('--- comment on #' + n + ' ---\n' + text + '\n---');
    const comments = await api('/repos/' + repo + '/issues/' + n + '/comments?per_page=100');
    const mine = comments.find(item => item.user?.login === BOT && item.body.includes(MARKER));
    if (!mine) await api('/repos/' + repo + '/issues/' + n + '/comments', { method: 'POST', body: JSON.stringify({ body: text }) });
    else if (mine.body !== text) await api('/repos/' + repo + '/issues/comments/' + mine.id, { method: 'PATCH', body: JSON.stringify({ body: text }) });
  }

  if (!opts.dryRun) {
    git(['config', 'user.name', BOT]);
    git(['config', 'user.email', BOT_EMAIL]);
  }

  // The table and docs are recomputed from the tip of main on every attempt, so two issues
  // handled at the same time cannot overwrite each other.
  let result;
  let table;
  pushWithRetry(
    () => {
      if (!opts.dryRun) {
        git(['fetch', '--depth=1', 'origin', 'main']);
        git(['reset', '--hard', 'FETCH_HEAD']);
      }
      table = JSON.parse(readFileSync(dataFile, 'utf8'));
      result = evaluate({ issue: n, body: issue.body, table, today: opts.today });
    },
    () => {
      if (result.status !== 'ok' || result.action === 'unchanged') return;
      const docs = applyToDocs(readFileSync(docsFile, 'utf8'), table);
      if (opts.dryRun) {
        console.log('--- ' + opts.docs + ' block ---\n' + docs.slice(docs.indexOf('<!-- device-results:start'), docs.indexOf('<!-- device-results:end -->')) + '---');
        return;
      }
      writeFileSync(dataFile, JSON.stringify(table, null, 2) + '\n');
      writeFileSync(docsFile, docs);
      const message = join(mkdtempSync(join(tmpdir(), 'commit-')), 'message.txt');
      const verb = result.action === 'updated' ? 'update' : 'add';
      writeFileSync(message, 'docs(devices): ' + verb + ' results from issue #' + n + '\n\nSigned-off-by: ' + BOT + ' <' + BOT_EMAIL + '>\n');
      git(['add', opts.data, opts.docs]);
      git(['commit', '-F', message]);
      git(['push', 'origin', 'HEAD:main']);
    }
  );

  if (result.status === 'invalid') {
    console.log('#' + n + ': invalid, ' + result.problems.length + ' problem(s)');
    await comment(failureComment(result.problems));
    return;
  }
  console.log('#' + n + ': ' + result.action + (result.hold ? ', kept open for the stream notes' : ''));
  if (opts.dryRun) console.log('Would copy ' + opts.data + ' to gh-pages:data/device-results.json');
  else publishToPages(dataFile);
  await comment(successComment(result));
  if (result.hold) return;
  if (opts.dryRun) return console.log('Would close #' + n + ' as completed');
  await api('/repos/' + repo + '/issues/' + n, { method: 'PATCH', body: JSON.stringify({ state: 'closed', state_reason: 'completed' }) });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch(error => {
    console.error('Error: ' + error.message);
    process.exitCode = 1;
  });
}
