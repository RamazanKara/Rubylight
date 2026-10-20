<script lang="ts">
  import { hostChecks } from '../../lib/host-readiness';
  import Badge from '../Badge.svelte';
  import Button from '../Button.svelte';
  import Panel from '../Panel.svelte';
  import type { Metadata } from '../../lib/api';
  import { link } from '../../lib/router.svelte';

  let {
    metadata,
    error,
    checking,
    onrecheck,
  }: { metadata: Metadata | null; error: string | null; checking: boolean; onrecheck: () => void } = $props();

  const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;
  const checks = $derived(metadata ? hostChecks(metadata) : []);
  const attention = $derived(checks.filter((check) => check.tone === 'warn' || check.tone === 'danger').length);
  const summary = $derived(
    !metadata
      ? undefined
      : attention
        ? `${plural(attention, 'check needs', 'checks need')} attention.`
        : metadata.encoder_status.state === 'checking' ? 'Checking the host…' : 'Ready to stream.',
  );
</script>

<Panel title="Host readiness" description={summary} flush>
  {#snippet actions()}
    <Button size="sm" variant="ghost" icon="refresh" busy={checking} onclick={onrecheck}>Recheck</Button>
  {/snippet}
  {#if error}
    <p class="notice danger error" role="alert">Host details didn't load: {error}</p>
  {/if}
  {#if !metadata}
    {#if !error}<p class="placeholder muted">Checking the host…</p>{/if}
  {:else}
    <ul>
      {#each checks as check (check.title)}
        {@const problem = check.tone === 'warn' || check.tone === 'danger'}
        <li>
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
          <a href={check.href} use:link>{check.settings}</a>
        </li>
      {/each}
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
  a {
    justify-self: start;
    margin-top: 2px;
    color: var(--muted);
    font-size: var(--text-xs);
  }
  a:hover {
    color: var(--ink);
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
