<script lang="ts">
  import Button from '../Button.svelte';
  import { indexOfId, markedRange, pieces, type LogLine } from './logtext';

  let {
    lines,
    pattern,
    matched,
    current,
    placeholder,
    follow = $bindable(true),
  }: {
    lines: LogLine[];
    pattern: RegExp | null;
    /** Ids of the lines that contain a match. Their matches are marked near the view and on the current match's line. */
    matched: Set<number>;
    current: { id: number; n: number } | null;
    /** Shown instead of the lines when set. */
    placeholder: string;
    /** Keep the newest line in view. */
    follow?: boolean;
  } = $props();

  /** Lines either side of the one in view whose matches are marked. */
  const MARK_SPAN = 200;
  /** How far the view moves before other lines are marked: more than half a tall window stays covered. */
  const MARK_SLACK = 60;

  let scroller = $state<HTMLDivElement>();
  let height = $state(480);
  /** Id of the line the marked lines are centred on. */
  let anchor = $state(0);
  const marked = $derived(markedRange(lines, anchor, MARK_SPAN));

  // Moves the marked lines to the line in the middle of the view, once it has moved far enough.
  function locate() {
    if (!scroller || !pattern) return;
    const box = scroller.getBoundingClientRect();
    const line = document.elementFromPoint(box.left + 24, box.top + box.height / 2)?.closest<HTMLElement>('.line');
    const id = Number(line?.dataset.id);
    if (!id || !scroller.contains(line ?? null)) return;
    if (Math.abs(indexOfId(lines, id) - indexOfId(lines, anchor)) > MARK_SLACK) anchor = id;
  }

  // Fill the window below the toolbar; the page itself should not scroll.
  function fit() {
    if (!scroller) return;
    const top = scroller.getBoundingClientRect().top + window.scrollY;
    const main = scroller.closest('main');
    const bottom = main ? parseFloat(getComputedStyle(main).paddingBottom) || 0 : 24;
    height = Math.max(280, Math.floor(window.innerHeight - top - bottom));
  }

  $effect(() => {
    // Content above (a pairing request, a wrapped toolbar) moves the viewer.
    const main = scroller?.closest('main');
    if (!main) return;
    let frame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(fit);
    });
    observer.observe(main);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  });

  $effect(() => {
    void lines;
    void height;
    void pattern;
    if (follow && scroller) scroller.scrollTop = scroller.scrollHeight;
    // New lines push the old ones up without a scroll event.
    locate();
  });

  $effect(() => {
    if (current && scroller) scroller.querySelector('mark.current')?.scrollIntoView({ block: 'center', inline: 'nearest' });
  });

  let locating = 0;

  function onscroll() {
    if (!scroller) return;
    const atBottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 8;
    if (atBottom !== follow) follow = atBottom;
    cancelAnimationFrame(locating);
    locating = requestAnimationFrame(locate);
  }

  $effect(() => () => cancelAnimationFrame(locating));
</script>

<svelte:window onresize={fit} />

{#snippet highlighted(line: LogLine, pattern: RegExp)}{#each pieces(line, pattern, current?.id === line.id ? current.n : 0) as piece}{#if piece.hit >= 0}<mark class:current={current?.id === line.id && current.n === piece.hit}>{piece.text}</mark>{:else if piece.time}<span class="time">{piece.text}</span>{:else}{piece.text}{/if}{/each}{/snippet}

<div class="frame">
  <!-- role="log" is a live region by default; announcing every line each second would drown out everything else. -->
  <div class="viewer" bind:this={scroller} {onscroll} style:height="{height}px" role="log" aria-live="off" aria-label="Host log">
    {#if placeholder}
      <p class="placeholder">{placeholder}</p>
    {:else}
      <div class="lines">
        <!-- Lines keep their spacing (white-space: pre), so nothing may sit between these tags. -->
        {#each lines as line (line.id)}
          <div class="line" class:warn={line.level === 'WARN'} class:error={line.level === 'ERROR'} data-id={line.id}>{#if pattern && matched.has(line.id) && ((line.id >= marked.from && line.id <= marked.to) || current?.id === line.id)}{@render highlighted(line, pattern)}{:else}<span class="time">{line.text.slice(0, line.time)}</span>{line.text.slice(line.time)}{/if}</div>
        {/each}
      </div>
    {/if}
  </div>
  {#if !follow && !placeholder}
    <div class="jump">
      <Button size="sm" variant="primary" onclick={() => (follow = true)}>Jump to latest</Button>
    </div>
  {/if}
</div>

<style>
  .frame {
    position: relative;
    min-width: 0;
  }
  .viewer {
    overflow: auto;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.6;
    overscroll-behavior: contain;
  }
  .lines {
    width: max-content;
    min-width: 100%;
    padding: var(--space-2) 0;
  }
  .line {
    white-space: pre;
    min-height: 1.6em;
    padding: 0 var(--space-3);
  }
  .line:hover {
    background: var(--sunken);
  }
  .warn {
    color: var(--warn);
  }
  .error {
    color: var(--danger);
  }
  .time {
    color: var(--muted);
  }
  mark {
    background: var(--accent-soft);
    color: inherit;
    border-radius: 2px;
    outline: 1px solid var(--accent);
  }
  mark.current {
    background: var(--accent);
    color: var(--accent-ink);
  }
  /* Centred in the empty viewer, so it reads as a state and not as a log line. */
  .placeholder {
    display: grid;
    place-items: center;
    min-height: 100%;
    padding: var(--space-5);
    text-align: center;
    color: var(--muted);
    font-family: var(--font);
    font-size: var(--text-sm);
  }
  @media (max-width: 520px) {
    .lines {
      width: 100%;
    }
    .line {
      white-space: pre-wrap;
      overflow-wrap: anywhere;
    }
  }
  .jump {
    position: absolute;
    right: var(--space-4);
    bottom: var(--space-4);
  }
</style>
