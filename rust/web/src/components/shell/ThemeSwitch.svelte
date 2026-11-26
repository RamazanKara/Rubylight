<script lang="ts">
  /** The shell's theme choice: System, Light and Dark in one pill. The shell keeps the choice. */
  import Icon, { type IconName } from '../Icon.svelte';
  import type { Theme } from '../../lib/theme';

  interface Props {
    theme: Theme;
    onchange: (theme: Theme) => void;
  }
  let { theme, onchange }: Props = $props();

  const themes: { value: Theme; label: string; icon: IconName }[] = [
    { value: 'system', label: 'System', icon: 'monitor' },
    { value: 'light', label: 'Light', icon: 'sun' },
    { value: 'dark', label: 'Dark', icon: 'moon' },
  ];
</script>

<div class="theme" role="group" aria-label="Theme">
  {#each themes as option (option.value)}
    <button
      type="button"
      aria-pressed={theme === option.value}
      aria-label="{option.label} theme"
      title="Theme: {option.label}"
      onclick={() => onchange(option.value)}
    >
      <Icon name={option.icon} size={17} />
    </button>
  {/each}
</div>

<style>
  .theme {
    flex: none;
    display: grid;
    grid-template-columns: repeat(3, minmax(var(--control-height), 1fr));
    padding: 0;
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: var(--radius-pill);
    background: var(--panel);
    box-shadow: var(--edge);
  }
  button {
    display: grid;
    place-items: center;
    height: calc(var(--control-height) - 2px);
    padding: 0;
    border: 0;
    border-radius: var(--radius-pill);
    background: none;
    color: var(--muted);
    cursor: pointer;
    transition:
      background var(--duration-fast),
      color var(--duration-fast);
  }
  button:hover {
    color: var(--ink);
  }
  button:focus-visible {
    outline-offset: -3px;
  }
  button[aria-pressed='true'] {
    background: var(--accent-soft);
    color: var(--accent-text);
  }
</style>
