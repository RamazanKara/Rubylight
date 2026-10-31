<script lang="ts">
  // The frame for the pages shown before the console: sign in, first-run setup
  // and "cannot connect". The brand above one centred card, on the page colour
  // with a soft ruby glow at the top.
  import type { Snippet } from 'svelte';
  import BrandMark from './BrandMark.svelte';

  let { children, footer }: { children: Snippet; footer?: Snippet } = $props();
</script>

<div class="auth">
  <div class="column">
    <div class="brand"><BrandMark size={40} /><span>Rubylight</span></div>
    <div class="card">{@render children()}</div>
    {#if footer}<div class="footer">{@render footer()}</div>{/if}
  </div>
</div>

<style>
  .auth {
    min-height: 100vh;
    min-height: 100dvh;
    display: grid;
    place-items: center;
    padding: var(--space-7) var(--space-4);
    background: radial-gradient(120% 60% at 50% 0%, var(--accent-soft), transparent 70%) no-repeat, var(--bg);
  }
  .column {
    width: min(440px, 100%);
    display: grid;
    gap: var(--space-5);
  }
  .brand {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-3);
    font-family: var(--font-display);
    font-weight: var(--weight-bold);
    font-size: var(--text-lg);
    letter-spacing: -0.01em;
  }
  .card {
    display: grid;
    gap: var(--space-4);
    padding: var(--space-6);
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-raised);
  }
  .card > :global(form) {
    display: grid;
    gap: var(--space-4);
  }
  .card :global(h1) {
    font-size: var(--text-xl);
  }
  .card :global(h1 + p) {
    margin-top: calc(var(--space-2) * -1);
    margin-bottom: var(--space-1);
    color: var(--muted);
  }
  .footer {
    text-align: center;
    font-size: var(--text-xs);
    color: var(--muted);
  }
  @media (max-width: 520px) {
    .auth {
      place-items: start center;
      padding: var(--space-6) var(--space-4);
    }
    .card {
      padding: var(--space-5);
    }
  }
</style>
