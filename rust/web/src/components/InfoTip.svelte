<script lang="ts">
  import Icon from './Icon.svelte';

  // The finer points of a label, in a popover beside it: an (i) that opens on
  // click, tap or Enter and closes on Escape, a click elsewhere or scrolling.
  let { label, text }: { label: string; text: string } = $props();

  const uid = $props.id();
  const id = `${uid}-info`;
  let button = $state<HTMLButtonElement>();
  let tip = $state<HTMLDivElement>();
  let open = $state(false);

  const EDGE = 12;
  const GAP = 6;

  function close() {
    tip?.hidePopover();
  }

  /** Below the (i), kept inside the window; above it when there is no room below. */
  function place() {
    if (!button || !tip) return;
    const anchor = button.getBoundingClientRect();
    const width = tip.offsetWidth;
    const height = tip.offsetHeight;
    const left = Math.max(EDGE, Math.min(anchor.left - 8, window.innerWidth - width - EDGE));
    const below = anchor.bottom + GAP;
    const top = below + height > window.innerHeight - EDGE && anchor.top - GAP - height > EDGE ? anchor.top - GAP - height : below;
    tip.style.left = `${left}px`;
    tip.style.top = `${top}px`;
  }

  function toggled(event: ToggleEvent) {
    open = event.newState === 'open';
    if (open) place();
  }

  $effect(() => {
    if (!open) return;
    // A fixed popover would part from its label: scrolling or resizing closes it.
    window.addEventListener('scroll', close, { capture: true, passive: true });
    window.addEventListener('resize', close);
    return () => {
      window.removeEventListener('scroll', close, { capture: true });
      window.removeEventListener('resize', close);
    };
  });
</script>

<button
  bind:this={button}
  type="button"
  class="info"
  class:open
  popovertarget={id}
  aria-expanded={open}
  aria-controls={id}
  aria-label="About {label}"
  title="About {label}"
>
  <Icon name="info" size={16} />
</button>
<div bind:this={tip} {id} class="tip" popover="auto" role="note" aria-label="About {label}" onbeforetoggle={(event) => { if (event.newState === 'open') place(); }} ontoggle={toggled}>
  {text}
</div>

<style>
  /* A 44 px target around a 24 px mark, taking no more room in the line than the mark. */
  .info {
    position: relative;
    display: inline-grid;
    place-items: center;
    flex: none;
    width: var(--control-height);
    height: var(--control-height);
    min-width: 0;
    min-height: 0;
    /* In a line of text it follows the last word, 6 px after it. */
    margin: calc((24px - var(--control-height)) / 2);
    margin-inline-start: calc((24px - var(--control-height)) / 2 + 6px);
    vertical-align: middle;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: none;
    color: var(--muted);
    cursor: pointer;
    transition: color var(--duration-fast);
  }
  .info::before {
    content: '';
    position: absolute;
    inset: calc((var(--control-height) - 24px) / 2);
    border-radius: 50%;
    transition: background var(--duration-fast);
  }
  .info :global(svg) {
    position: relative;
  }
  .info:hover,
  .info.open {
    color: var(--ink);
  }
  .info:hover::before,
  .info.open::before {
    background: var(--sunken);
  }
  /* The console's focus ring, around the mark rather than the whole target. */
  .info:focus-visible {
    outline: none;
  }
  .info:focus-visible::before {
    outline: var(--focus-width) solid var(--focus);
    outline-offset: var(--focus-offset);
  }
  .tip {
    inset: auto;
    margin: 0;
    width: max-content;
    max-width: min(22rem, calc(100vw - 24px));
    max-height: min(60vh, 24rem);
    overflow: auto;
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel-raised);
    color: var(--ink-2);
    box-shadow: var(--shadow-pop);
    font-size: var(--text-sm);
    line-height: var(--leading);
    overflow-wrap: break-word;
  }
</style>
