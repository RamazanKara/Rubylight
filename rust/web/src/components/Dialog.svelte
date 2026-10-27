<script lang="ts">
  // A modal dialog on the native <dialog>: the browser traps focus, Escape
  // closes it and focus returns to the control that opened it.
  import type { Snippet } from 'svelte';

  let {
    open = $bindable(false),
    title,
    description,
    size = 'md',
    dismissible = true,
    onclose,
    children,
    actions,
  }: {
    open?: boolean;
    title: string;
    description?: string;
    size?: 'sm' | 'md' | 'lg';
    /** Escape and a click on the backdrop close it. Off while something is saving. */
    dismissible?: boolean;
    onclose?: () => void;
    children?: Snippet;
    /** Buttons at the bottom, primary last. */
    actions?: Snippet;
  } = $props();

  const id = $props.id();
  let element = $state<HTMLDialogElement>();

  $effect(() => {
    if (!element) return;
    if (open && !element.open) element.showModal();
    if (!open && element.open) element.close();
  });

  function closed() {
    if (!open) return;
    open = false;
    onclose?.();
  }
  function cancel(event: Event) {
    if (!dismissible) event.preventDefault();
  }
  function backdrop(event: MouseEvent) {
    if (dismissible && event.target === element) element?.close();
  }
</script>

<dialog
  bind:this={element}
  class={size}
  aria-labelledby="{id}-title"
  aria-describedby={description ? `${id}-description` : undefined}
  oncancel={cancel}
  onclose={closed}
  onclick={backdrop}
>
  {#if open}
    <div class="content">
      <header>
        <h2 id="{id}-title">{title}</h2>
        {#if description}<p id="{id}-description">{description}</p>{/if}
      </header>
      {#if children}<div class="body">{@render children()}</div>{/if}
      {#if actions}<div class="actions">{@render actions()}</div>{/if}
    </div>
  {/if}
</dialog>

<style>
  dialog {
    width: min(var(--dialog-width), calc(100vw - var(--space-6)));
    max-height: calc(100dvh - var(--space-7));
    padding: 0;
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--panel-raised);
    color: var(--ink);
    box-shadow: var(--shadow-pop);
  }
  .sm {
    --dialog-width: 400px;
  }
  .md {
    --dialog-width: 520px;
  }
  .lg {
    --dialog-width: 760px;
  }
  dialog[open] {
    animation: enter var(--duration-base) var(--ease-enter);
  }
  dialog::backdrop {
    background: var(--scrim);
  }
  .content {
    display: grid;
    gap: var(--space-4);
    padding: var(--space-5);
  }
  h2 {
    font-size: var(--text-lg);
  }
  header p {
    margin-top: var(--space-2);
    color: var(--ink-2);
    white-space: pre-line;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: var(--space-2);
    padding-top: var(--space-1);
  }
  @keyframes enter {
    from {
      opacity: 0;
      transform: translateY(8px) scale(0.98);
    }
  }
</style>
