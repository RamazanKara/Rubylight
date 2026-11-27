<script lang="ts">
  import Badge from '../Badge.svelte';
  import Icon from '../Icon.svelte';
  import type { LogAnalysis } from '../../lib/api';
  import { settingHref } from '../../lib/quick-switch';
  import { link } from '../../lib/router.svelte';
  import { schema } from '../../lib/settings-schema';
  import { localTime } from './util';

  let { analysis }: { analysis: LogAnalysis } = $props();

  /** The setting a fix names, as All settings shows it; a key the console doesn't describe gets no link. */
  const settingOf = (key: string | null | undefined) => (key ? schema.settings.find((setting) => setting.key === key) : undefined);

  const categories = { encoder: 'Encoder', capture: 'Capture', network: 'Network', driver: 'Driver' } as const;
</script>

{#if analysis.findings.length}
  <ul class="findings">
    {#each analysis.findings as finding (finding.id)}
      {@const setting = settingOf(finding.setting)}
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
        <div class="links">
          {#if setting}<a class="small" href={settingHref(setting)} use:link>Open {setting.label}</a>{/if}
          <a class="small" href={finding.link} target="_blank" rel="noreferrer">How to fix it<Icon name="external" size={14} /></a>
        </div>
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
  .links {
    display: flex;
    flex-wrap: wrap;
    column-gap: var(--space-4);
  }
  a {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    margin-top: var(--space-1);
  }
</style>
