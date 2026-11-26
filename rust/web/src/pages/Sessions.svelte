<script lang="ts">
  import { onMount } from 'svelte';
  import Badge from '../components/Badge.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import Icon from '../components/Icon.svelte';
  import LiveCommandStage from '../components/overview/LiveCommandStage.svelte';
  import LoadState from '../components/LoadState.svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import Panel from '../components/Panel.svelte';
  import Table from '../components/Table.svelte';
  import { api, CODEC_NAMES, type SessionHistoryEntry, type StreamSession } from '../lib/api';
  import { confirm, failed, notify } from '../lib/feedback.svelte';
  import { ago, duration, marked, poll, when } from '../lib/format';
  import {
    aStreamEnded,
    bitrateText,
    categoryLabel,
    durationText,
    fpsText,
    lossText,
    matches,
    p95Text,
    pictureText,
    reasonLabel,
    reasonTone,
  } from '../lib/session-history';

  let streams = $state<StreamSession[] | null>(null);
  let streamsError = $state('');
  let history = $state<SessionHistoryEntry[] | null>(null);
  let historyError = $state('');
  // The host's caps, so the page names them rather than repeating them.
  let caps = $state<{ limit: number; days: number } | null>(null);
  let query = $state('');

  const shown = $derived(history ? history.filter((entry) => matches(entry, query)) : []);
  /** The streams to devices: they get the live command stage, as on Home. */
  const staged = $derived((streams ?? []).filter((stream) => stream.role === 'stream'));
  const others = $derived((streams ?? []).filter((stream) => stream.role !== 'stream'));
  /** uuid of the device being disconnected. */
  let disconnecting = $state<string | null>(null);

  async function disconnect(target: StreamSession) {
    const name = target.device_name || 'this device';
    const ok = await confirm({
      title: `Disconnect ${name}?`,
      message: 'Every connection from this device ends. The running app stays open, and the device can connect again.',
      confirm: 'Disconnect',
    });
    if (!ok) return;
    disconnecting = target.uuid;
    try {
      await api.clients.disconnect(target.uuid);
      notify(`Disconnected ${name}.`, 'ok');
      await refreshStreams();
    } catch (failure) {
      failed(`Disconnecting ${name} failed`, failure);
    } finally {
      disconnecting = null;
    }
  }

  function reason(value: unknown) {
    return value instanceof Error ? value.message : String(value);
  }

  async function loadHistory() {
    try {
      const reply = await api.sessions.history();
      history = reply.sessions;
      caps = { limit: reply.limit, days: reply.days };
      historyError = '';
    } catch (failure) {
      historyError = reason(failure);
    }
  }

  // Only the newest poll may land, as Home's stream poll does.
  let latest = 0;
  async function refreshStreams() {
    const id = ++latest;
    try {
      const reply = await api.sessions.streams();
      if (id !== latest) return;
      const next = reply.sessions ?? [];
      const ended = aStreamEnded(streams, next);
      streams = next;
      streamsError = '';
      // A stream that just left is in the history by now; the slower history
      // poll covers a write that lands after this one.
      if (ended) void marked(loadHistory);
    } catch (failure) {
      if (id !== latest) return;
      streamsError = reason(failure);
    }
  }

  onMount(() => {
    // The first history load is the poll's own; a stream that ends is reloaded
    // at once above, and this slower tick catches anything else.
    const stopHistory = poll(loadHistory, 15_000);
    const stopStreams = poll(refreshStreams, 2000);
    return () => {
      stopHistory();
      stopStreams();
    };
  });
</script>

<div class="page">
  <PageHeader
    title="Sessions"
    subtitle={caps
      ? `Every stream this PC ran, newest first: up to the last ${caps.limit}, kept for ${caps.days} days.`
      : 'Every stream this PC ran, with how it went, newest first.'}
  />

  {#if staged.length}
    <LiveCommandStage label="Live now" streams={staged} stale={Boolean(streamsError)} {disconnecting} ondisconnect={disconnect} />
  {/if}
  {#if !staged.length || others.length}
  <Panel title={staged.length ? 'Other connections' : 'Live now'} description={!staged.length && streams?.length ? `${streams.length} streaming` : undefined} flush>
    {#if streams === null}
      <div class="pad"><LoadState error={streamsError} what="the live streams" onretry={refreshStreams} /></div>
    {:else if streamsError}
      <div class="pad"><LoadState error={streamsError} what="the live streams" stale onretry={refreshStreams} /></div>
    {:else if streams.length === 0}
      <p class="pad muted">Nothing is streaming. A stream shows here while it runs, then in the history below.</p>
    {:else}
      <ul class="live">
        {#each staged.length ? others : streams as stream, index (index)}
          <li>
            <div class="who">
              <span class="name">{stream.device_name || 'Unnamed device'}</span>
              {#if stream.state === 'STOPPING'}
                <Badge tone="warn">Stopping</Badge>
              {:else}
                <Badge tone="live">Live</Badge>
              {/if}
              {#if stream.guest != null}<Badge>Guest</Badge>{/if}
            </div>
            <span class="numbers">
              {CODEC_NAMES[stream.video_format] ?? 'Unknown video format'}
              {#if stream.width && stream.height}<span class="num">{stream.width}×{stream.height}</span>{/if}
              <span class="num">{fpsText(stream.performance?.fps ?? 0)}</span>
              <span class="num">{(stream.encoder_bitrate_kbps / 1000).toFixed(1)} Mbps</span>
              <span>for <span class="num">{duration(stream.uptime_seconds)}</span></span>
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  </Panel>
  {/if}

  <Panel
    title="History"
    description={history ? (query ? `Showing ${shown.length} of ${history.length}` : `${history.length} sessions`) : undefined}
    flush
  >
    <!-- Nothing to search once the history is known to be empty. -->
    {#if history === null || history.length > 0}
      <div class="toolbar">
        <label class="search">
          <Icon name="search" size={16} />
          <span class="visually-hidden">Search sessions</span>
          <input type="search" placeholder="Search a device or game" bind:value={query} autocomplete="off" />
        </label>
      </div>
    {/if}
    {#if history === null}
      <div class="pad"><LoadState error={historyError} what="the session history" onretry={loadHistory} /></div>
    {:else if historyError}
      <div class="pad"><LoadState error={historyError} what="the session history" stale onretry={loadHistory} /></div>
    {:else if history.length === 0}
      <EmptyState icon="logs" title="No sessions yet">
        <p>Streams appear here once they end, with how they ran and why they stopped.</p>
      </EmptyState>
    {:else if shown.length === 0}
      <EmptyState icon="search" title="No matching sessions">
        <p>Nothing in the history matches “{query}”.</p>
      </EmptyState>
    {:else}
      <Table caption="Ended stream sessions">
        <thead>
          <tr>
            <th>Ended</th>
            <th>Session</th>
            <th>Picture</th>
            <th class="numeric">Frame rate</th>
            <th class="numeric">Bitrate</th>
            <th class="numeric">Loss</th>
            <th class="numeric">Duration</th>
            <th>How it ended</th>
          </tr>
        </thead>
        <tbody>
          {#each shown as entry (entry.id)}
            <tr>
              <td class="when"><span title={when(entry.ended_unix_ms / 1000)}>{ago(entry.ended_unix_ms / 1000)}</span></td>
              <td class="session">
                <span class="device">{entry.device_name || 'Unknown device'}</span>
                <span class="game">{entry.app || 'No game'}</span>
              </td>
              <td class="picture">{pictureText(entry)}</td>
              <td class="numeric">
                <span class="value">{fpsText(entry.mean_fps)}</span>
                <span class="sub">p95 {p95Text(entry.p95_fps)}</span>
              </td>
              <td class="numeric">{bitrateText(entry)}</td>
              <td class="numeric">{lossText(entry)}</td>
              <td class="numeric">{durationText(entry)}</td>
              <td class="ended">
                <Badge tone={reasonTone(entry.end_reason)}>{reasonLabel(entry.end_reason)}</Badge>
                {#if categoryLabel(entry.end_category)}
                  <span class="category">{categoryLabel(entry.end_category)}</span>
                {/if}
                {#if entry.end_detail}
                  <details class="detail">
                    <summary>Why</summary>
                    <p>{entry.end_detail}</p>
                  </details>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </Table>
      <p class="footnote muted">
        Bitrate is what the stream sent on average; when the stream ended below the rate it asked for
        (the network, the client or the host's limit held it there), the requested rate follows. p95 is the frame rate the slowest 5% of sampled seconds ran at or below. Loss is what the
        device reported missing, as a share of the video the host sent.
      </p>
    {/if}
  </Panel>
</div>

<style>
  .page {
    display: grid;
    gap: var(--space-5);
    min-width: 0;
  }
  .pad {
    padding: var(--space-4) var(--space-5);
  }
  .live {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .live li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: var(--space-2) var(--space-4);
    padding: var(--space-3) var(--space-5);
    border-bottom: 1px solid var(--line);
  }
  .live li:last-child {
    border-bottom: 0;
  }
  .who {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-2);
    min-width: 0;
  }
  .name {
    font-weight: var(--weight-medium);
    overflow-wrap: anywhere;
  }
  .numbers {
    display: flex;
    flex-wrap: wrap;
    gap: 2px var(--space-4);
    color: var(--ink-2);
    font-size: var(--text-sm);
  }
  .toolbar {
    padding: var(--space-3) var(--space-5) 0;
  }
  .search {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    max-width: 340px;
    padding: 0 var(--space-3);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
    color: var(--muted);
  }
  .search input {
    flex: 1 1 auto;
    min-width: 0;
    height: var(--control-height);
    border: 0;
    background: none;
    color: var(--ink);
    font: inherit;
  }
  .search input:focus-visible {
    outline: none;
  }
  .search:focus-within {
    outline: var(--focus-width) solid var(--focus);
    outline-offset: var(--focus-offset);
  }
  .device,
  .game {
    display: block;
  }
  .device {
    font-weight: var(--weight-medium);
  }
  .game {
    font-size: var(--text-sm);
    color: var(--muted);
  }
  .session {
    min-width: 16ch;
  }
  .picture {
    min-width: 20ch;
  }
  .when {
    min-width: 11ch;
  }
  /* A row stays one line where it can: on a phone the table scrolls sideways
     rather than stacking a codec name one syllable per line. */
  th,
  td {
    white-space: nowrap;
  }
  .session,
  .picture,
  .ended,
  .detail p {
    white-space: normal;
  }
  .value {
    display: block;
  }
  .sub {
    display: block;
    font-size: var(--text-xs);
    color: var(--muted);
  }
  .ended {
    min-width: 11ch;
  }
  .category {
    display: inline-block;
    margin-left: var(--space-2);
    font-size: var(--text-xs);
    color: var(--muted);
  }
  .detail {
    margin-top: var(--space-1);
  }
  .detail summary {
    font-size: var(--text-xs);
    color: var(--muted);
  }
  .detail p {
    margin: var(--space-1) 0 0;
    max-width: 44ch;
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
  .footnote {
    margin: 0;
    padding: var(--space-3) var(--space-5);
    border-top: 1px solid var(--line);
    font-size: var(--text-xs);
  }
  @media (max-width: 520px) {
    .live li,
    .pad,
    .toolbar,
    .footnote {
      padding-left: var(--space-4);
      padding-right: var(--space-4);
    }
  }
</style>
