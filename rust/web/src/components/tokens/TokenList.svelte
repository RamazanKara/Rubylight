<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import Button from '../Button.svelte';
  import EmptyState from '../EmptyState.svelte';
  import LoadState from '../LoadState.svelte';
  import Panel from '../Panel.svelte';
  import { api, type ApiToken } from '../../lib/api';
  import { confirm, failed, notify } from '../../lib/feedback.svelte';
  import { when } from '../../lib/format';
  import { permissions } from './scopes';

  let {
    tokens,
    error,
    onchange,
  }: { tokens: ApiToken[] | null; error: string; onchange: () => Promise<void> } = $props();

  const SUMMARY = 3;
  const expanded = new SvelteSet<string>();
  let revoking = $state<string | null>(null);

  const created = (value: number | string) => (typeof value === 'number' ? when(value) : value);

  async function revoke(token: ApiToken) {
    const ok = await confirm({
      title: 'Revoke this token?',
      message: `Scripts using the token created ${created(token.created_at)} lose access immediately. This cannot be undone.`,
      confirm: 'Revoke',
      danger: true,
    });
    if (!ok) return;
    revoking = token.hash;
    try {
      await api.tokens.revoke(token.hash);
      notify('Token revoked.', 'ok');
      await onchange();
    } catch (failure) {
      failed('Revoking the token failed', failure);
    } finally {
      revoking = null;
    }
  }
</script>

<Panel title="Tokens" description={tokens?.length ? `${tokens.length} ${tokens.length === 1 ? 'token' : 'tokens'}` : undefined} flush>
  {#if tokens && error}
    <!-- A failed refresh (after a revoke, say) must not pass the old list off as current. -->
    <div class="pad"><LoadState {error} what="the tokens" stale onretry={onchange} /></div>
  {/if}
  {#if tokens === null}
    <div class="pad"><LoadState {error} what="the tokens" onretry={onchange} /></div>
  {:else if tokens.length === 0}
    <EmptyState icon="key" title="No API tokens">
      <p>Create one below for each script that needs access.</p>
    </EmptyState>
  {:else}
    <ul>
      {#each tokens as token (token.hash)}
        {@const list = permissions(token.scopes)}
        {@const open = expanded.has(token.hash)}
        <li>
          <div class="info">
            <div class="head">
              <strong>Created {created(token.created_at)}</strong>
            </div>
            <p class="scopes mono">
              {open ? list.join(', ') : list.slice(0, SUMMARY).join(', ')}{#if !open && list.length > SUMMARY}, …{/if}
            </p>
            {#if list.length > SUMMARY}
              <button
                class="more"
                aria-expanded={open}
                onclick={() => (open ? expanded.delete(token.hash) : expanded.add(token.hash))}
              >
                {open ? 'Show fewer' : `Show all ${list.length}`}
              </button>
            {/if}
          </div>
          <Button
            size="sm"
            variant="danger"
            busy={revoking === token.hash}
            disabled={revoking !== null}
            onclick={() => revoke(token)}
          >
            Revoke
          </Button>
        </li>
      {/each}
    </ul>
  {/if}
</Panel>

<style>
  .pad {
    padding: var(--space-4) var(--space-5);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: var(--space-4);
    padding: var(--space-3) var(--space-5);
    border-bottom: 1px solid var(--line);
  }
  li:last-child {
    border-bottom: 0;
  }
  .info {
    display: grid;
    gap: var(--space-1);
    min-width: 0;
    justify-items: start;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: var(--space-1) var(--space-3);
    font-size: var(--text-sm);
  }
  .head strong {
    font-weight: var(--weight-semibold);
  }
  .scopes {
    font-size: var(--text-xs);
    color: var(--ink-2);
    overflow-wrap: anywhere;
    line-height: 1.55;
  }
  .more {
    border: 0;
    background: none;
    padding: 0;
    color: var(--muted);
    font-size: var(--text-xs);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  .more:hover {
    color: var(--ink);
  }
  /* Lists reach the panel's edges, so they follow its narrower padding on a phone. */
  @media (max-width: 520px) {
    li,
    .pad {
      padding-left: var(--space-4);
      padding-right: var(--space-4);
    }
  }
</style>
