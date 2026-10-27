<script lang="ts">
  import Badge from '../Badge.svelte';
  import Icon from '../Icon.svelte';
  import type { LogAnalysis } from '../../lib/api';
  import { localTime } from './util';

  let { analysis }: { analysis: LogAnalysis } = $props();

  const categories = { encoder: 'Encoder', capture: 'Capture', network: 'Network', driver: 'Driver' } as const;
</script>

{#if analysis.findings.length}
  <ul class="findings">
    {#each analysis.findings as finding (finding.id)}
      <li>
        <div class="row">
          <strong>{finding.title}</strong>
          <Badge tone={finding.level === 'error' ? 'danger' : 'warn'}>{categories[finding.category] ?? finding.category}</Badge>
          <span class="muted small">{finding.count === 1 ? 'Once' : `${finding.count} times`}, last {localTime(finding.last_seen)}</span>
        </div>
        <p>{finding.explanation}</p>
        <p><span class="muted">Fix:</span> {finding.fix}</p>
        <details>
          <summary>Latest log line</summary>
          <p class="mono sample">{finding.sample}</p>
        </details>
        <a class="small" href={finding.link} target="_blank" rel="noreferrer">How to fix it<Icon name="external" size={14} /></a>
      </li>
    {/each}
  </ul>
{:else}
  <p class="muted note">No known problems in the log{analysis.since ? ` since ${localTime(analysis.since)}` : ''}.</p>
{/if}

<style>
  .findings {
    display: grid;
    gap: var(--space-4);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .findings > li + li {
    padding-top: var(--space-4);
    border-top: 1px solid var(--line);
  }
  .findings p {
    margin: var(--space-1) 0 0;
    max-width: 65ch;
    font-size: var(--text-sm);
  }
  .small,
  .note {
    font-size: var(--text-sm);
  }
  .sample {
    overflow-wrap: anywhere;
  }
  a {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-top: var(--space-1);
  }
</style>
