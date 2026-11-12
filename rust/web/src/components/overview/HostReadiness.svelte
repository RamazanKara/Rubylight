<script lang="ts">
  import { hostChecks, isProblem, problems, type Check } from '../../lib/host-readiness';
  import { applyFix, dismissWarning, fixes } from '../../lib/host-fixes.svelte';
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import type { Metadata, PowerStatus } from '../../lib/api';
  import { link } from '../../lib/router.svelte';

  let {
    metadata,
    power = null,
    error,
    checking,
    onrecheck,
    onfixed,
    skip,
  }: {
    metadata: Metadata | null;
    /** The wake status, for whether devices can wake this PC; null leaves that check out. */
    power?: PowerStatus | null;
    error: string | null;
    checking: boolean;
    onrecheck: () => void;
    /** A fix changed the PC: the new wake status when it was a wake fix. */
    onfixed?: (power: PowerStatus | null) => void;
    /** A problem the page already shows with its fix (Home's top), left out here. */
    skip?: string;
  } = $props();

  const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;
  // Problems only, each with its fix; what is fine folds into one line below them.
  const found = $derived(metadata ? problems(metadata, power, fixes.dismissed).filter((check) => check.id !== skip) : []);
  const fine = $derived(metadata ? hostChecks(metadata, power).filter((check) => !isProblem(check)) : []);
  const ready = $derived(fine.filter((check) => check.tone === 'ok').length);
  const rest = $derived(fine.filter((check) => check.tone !== 'ok').map((check) => `${check.title} ${check.state.toLowerCase()}`));
  const fineLabel = $derived(`${plural(ready, 'check', 'checks')} ready${rest.length ? ` · ${rest.join(', ')}` : ''}`);
  const summary = $derived(
    !metadata || found.length ? undefined : metadata.encoder_status.state === 'checking' ? 'Checking this PC…' : 'Ready to stream.',
  );
</script>

{#snippet row(check: Check, problem: boolean)}
  <li class:problem>
    <div class="head">
      <h3>{check.title}</h3>
      <Badge tone={check.tone}>{check.state}</Badge>
    </div>
    <p class="text">{check.text}</p>
    {#if check.help}<p class="help">{check.help}</p>{/if}
    {#if check.detail}
      <!-- The host's own words: open for a problem, a click away otherwise. -->
      <details open={problem}>
        <summary>Details</summary>
        <p class="detail">{check.detail}</p>
      </details>
    {/if}
    {#if check.fix || check.warning || check.href}
      <div class="row-actions">
        {#if check.fix}
          <Button size="sm" busy={fixes.running === check.id} disabled={Boolean(fixes.running)} onclick={() => applyFix(check, onfixed)}>
            {check.fix.label}<span class="visually-hidden">: {check.title}</span>
          </Button>
        {/if}
        {#if check.warning}
          <Button size="sm" variant="ghost" busy={fixes.running === check.id} disabled={Boolean(fixes.running)} onclick={() => dismissWarning(check, onrecheck)}>
            Dismiss<span class="visually-hidden"> {check.title}</span>
          </Button>
        {/if}
        {#if check.href}<a href={check.href} use:link>{check.settings}</a>{/if}
      </div>
    {/if}
  </li>
{/snippet}

<!-- Home's checks: only what needs a look, with the way to Help; a ready PC's checks on request. -->
<Panel title={found.length ? 'Needs attention' : 'Checks'} description={summary} flush>
  {#snippet actions()}
    {#if found.length}<Button size="sm" variant="ghost" icon="help" href="/help">Help</Button>{/if}
    <Button size="sm" variant="ghost" icon="refresh" busy={checking} onclick={onrecheck}>Recheck</Button>
  {/snippet}
  {#if error}
    <p class="notice danger error" role="alert">This PC's details didn't load: {error}</p>
  {/if}
  {#if !metadata}
    {#if !error}<p class="placeholder muted">Checking this PC…</p>{/if}
  {:else}
    <ul>
      {#each found as check (check.id)}{@render row(check, true)}{/each}
      {#if found.length && fine.length}
        <li class="fine">
          <details>
            <summary><span class="ok-dot" aria-hidden="true"></span>{fineLabel}</summary>
            <ul>{#each fine as check (check.id)}{@render row(check, false)}{/each}</ul>
          </details>
        </li>
      {:else}
        {#each fine as check (check.id)}{@render row(check, false)}{/each}
      {/if}
    </ul>
  {/if}
</Panel>

<style>
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  li {
    display: grid;
    gap: 2px;
    padding: var(--space-3) var(--space-5);
    font-size: var(--text-sm);
  }
  li + li {
    border-top: 1px solid var(--line);
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--space-2);
  }
  h3 {
    font-size: var(--text);
  }
  .text {
    color: var(--ink-2);
  }
  .help {
    color: var(--ink);
  }
  details {
    font-size: var(--text-xs);
    color: var(--muted);
  }
  summary {
    cursor: pointer;
    width: fit-content;
  }
  summary:hover {
    color: var(--ink);
  }
  .detail {
    margin-top: 2px;
    padding: var(--space-2) var(--space-3);
    border-left: 2px solid var(--line-strong);
    background: var(--sunken);
    color: var(--ink-2);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }
  .row-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2) var(--space-4);
    margin-top: var(--space-2);
  }
  a {
    color: var(--muted);
    font-size: var(--text-xs);
  }
  a:hover {
    color: var(--ink);
  }
  /* What is fine: one quiet line that opens to the checks. */
  .fine {
    padding-block: var(--space-2);
  }
  .fine > details {
    font-size: var(--text-sm);
    color: var(--ink-2);
  }
  .fine summary {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    min-height: var(--control-height);
  }
  .ok-dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ok);
  }
  .fine li {
    padding-inline: 0;
  }
  .fine ul {
    border-top: 1px solid var(--line);
  }
  .placeholder {
    padding: var(--space-4) var(--space-5);
    font-size: var(--text-sm);
  }
  .error {
    margin: var(--space-3) var(--space-5);
  }
  @media (max-width: 520px) {
    li {
      padding: var(--space-3) var(--space-4);
    }
    .placeholder {
      padding: var(--space-4);
    }
    .error {
      margin: var(--space-3) var(--space-4);
    }
  }
</style>
