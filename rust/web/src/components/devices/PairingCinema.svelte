<script lang="ts">
  /**
   * The pairing flow's frame: three steps on a rail and one sentence that says what to do now,
   * around the panel that does the pairing. The current step follows the host (a waiting PIN
   * request, a device that just paired), so the rail never runs ahead of what happened.
   */
  import type { Snippet } from 'svelte';
  import Icon from '../Icon.svelte';
  import { pairingLead, pairingStage, pairingSteps, type PairingFacts } from './pairing-steps';

  let { facts, children }: { facts: PairingFacts; children: Snippet } = $props();

  const stage = $derived(pairingStage(facts));
  const steps = $derived(pairingSteps(stage, facts.off));
  const lead = $derived(pairingLead(facts));
</script>

<div class="cinema {stage}">
  <ol class="rail" aria-label="Pairing steps">
    {#each steps as step, index (step.id)}
      <li class={step.state} aria-current={step.state === 'now' ? 'step' : undefined}>
        <span class="disc" aria-hidden="true">
          {#if step.state === 'done'}<Icon name="check" size={14} />{:else}{index + 1}{/if}
        </span>
        <span class="title">{step.title}{#if step.state === 'done'}<span class="visually-hidden">: done</span>{/if}</span>
      </li>
    {/each}
  </ol>
  <p class="lead">{lead}</p>
  {@render children()}
</div>

<style>
  .cinema {
    display: grid;
    gap: var(--space-4);
    min-width: 0;
  }
  .rail {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
    color: var(--muted);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
  }
  /* The line to the next step: it fills once this step is done. */
  li:not(:last-child) {
    flex: 1 1 0;
  }
  li:not(:last-child)::after {
    content: '';
    flex: 1 1 var(--space-5);
    min-width: var(--space-4);
    height: 2px;
    border-radius: var(--radius-pill);
    background: var(--line-strong);
    transition: background-color var(--duration-base) var(--ease-standard);
  }
  li.done:not(:last-child)::after {
    background: var(--ok);
  }
  .disc {
    display: grid;
    flex: none;
    place-items: center;
    width: 26px;
    height: 26px;
    border: 1px solid var(--line-strong);
    border-radius: 50%;
    background: var(--panel);
    color: var(--ink-2);
    font-family: var(--mono);
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
    transition: background-color var(--duration-base) var(--ease-standard), border-color var(--duration-base) var(--ease-standard);
  }
  .title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .now {
    color: var(--ink);
    font-weight: var(--weight-semibold);
  }
  .now .disc {
    border-color: var(--accent);
    background: var(--accent);
    box-shadow: var(--ring-accent);
    color: var(--accent-ink);
  }
  .done {
    color: var(--ink-2);
  }
  .done .disc {
    border-color: var(--ok);
    background: var(--ok);
    color: var(--panel);
  }
  .lead {
    max-width: 68ch;
    color: var(--ink-2);
  }
  .ready .lead {
    color: var(--ink);
  }
  /* A phone has room for one title: the current step's. The discs keep the count. */
  @media (max-width: 600px) {
    .rail {
      gap: var(--space-2);
    }
    .cinema:not(.ready) li:not(.now) .title,
    .cinema.ready li:not(:last-child) .title {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
    }
    li:not(:last-child) {
      flex: 0 1 auto;
    }
    li.now:not(:last-child) {
      flex: 1 1 auto;
    }
    li:not(:last-child)::after {
      flex-basis: var(--space-4);
    }
  }
</style>
