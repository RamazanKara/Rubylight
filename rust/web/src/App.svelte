<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Icon, { type IconName } from './components/Icon.svelte';
  import Button from './components/Button.svelte';
  import CopyButton from './components/CopyButton.svelte';
  import ConfirmHost from './components/ConfirmHost.svelte';
  import Toaster from './components/Toaster.svelte';
  import PairingBanner from './components/PairingBanner.svelte';
  import { api, setSignedOutHandler } from './lib/api';
  import { failed } from './lib/feedback.svelte';
  import { poll } from './lib/format';
  import { link, match, mayLeave, route } from './lib/router.svelte';
  import { checkSession, refreshPending, session, signedOut } from './lib/session.svelte';
  import { applyTheme, storedTheme, type Theme } from './lib/theme';
  import Overview from './pages/Overview.svelte';
  import Library from './pages/Library.svelte';
  import LibraryImport from './pages/LibraryImport.svelte';
  import Stream from './pages/Stream.svelte';
  import Devices from './pages/Devices.svelte';
  import Settings from './pages/Settings.svelte';
  import Logs from './pages/Logs.svelte';
  import Maintenance from './pages/Maintenance.svelte';
  import Tokens from './pages/Tokens.svelte';
  import Webhooks from './pages/Webhooks.svelte';
  import Login from './pages/Login.svelte';
  import Setup from './pages/Setup.svelte';
  import NotFound from './pages/NotFound.svelte';

  type NavItem = { path: string; label: string; icon: IconName; match?: (path: string) => boolean };
  // The everyday pages; Help below them; everything else in Advanced, folded.
  const primary: NavItem[] = [
    { path: '/', label: 'Home', icon: 'overview' },
    { path: '/library', label: 'Games', icon: 'library' },
    { path: '/devices', label: 'Devices', icon: 'devices' },
    { path: '/settings', label: 'Settings', icon: 'settings', match: (path) => path === '/settings' },
    { path: '/maintenance', label: 'Help', icon: 'help' },
  ];
  const advanced: NavItem[] = [
    { path: '/settings/general', label: 'All settings', icon: 'maintenance', match: (path) => path.startsWith('/settings/') },
    { path: '/stream', label: 'Stream in the browser', icon: 'play' },
    { path: '/logs', label: 'Logs', icon: 'logs' },
    { path: '/tokens', label: 'API tokens', icon: 'key' },
    { path: '/webhooks', label: 'Webhooks', icon: 'link' },
  ];
  const themes: { value: Theme; label: string; icon: IconName }[] = [
    { value: 'system', label: 'System', icon: 'monitor' },
    { value: 'light', label: 'Light', icon: 'sun' },
    { value: 'dark', label: 'Dark', icon: 'moon' },
  ];

  let theme = $state<Theme>(storedTheme());
  let menuOpen = $state(false);
  let signingOut = $state(false);
  let menuButton = $state<HTMLButtonElement>();
  let content = $state<HTMLElement>();
  let lastPath = route.path;

  const current = (item: NavItem) =>
    item.match
      ? item.match(route.path)
      : item.path === '/'
        ? route.path === '/'
        : route.path === item.path || route.path.startsWith(`${item.path}/`);
  const inAdvanced = $derived(advanced.some(current));
  let advancedOpen = $state(false);

  const Page = $derived.by(() => {
    if (route.path === '/login') return Login;
    if (route.path === '/setup') return Setup;
    if (route.path === '/') return Overview;
    if (route.path === '/library/import') return LibraryImport;
    if (match('/library') || match('/library/:uuid')) return Library;
    if (route.path === '/stream') return Stream;
    if (route.path === '/devices') return Devices;
    if (match('/settings') || match('/settings/:section')) return Settings;
    if (route.path === '/logs') return Logs;
    if (route.path === '/maintenance') return Maintenance;
    if (route.path === '/tokens') return Tokens;
    if (route.path === '/webhooks') return Webhooks;
    return NotFound;
  });
  const bare = $derived(route.path === '/login' || route.path === '/setup' || session.state !== 'signed-in');

  function setTheme(next: Theme) {
    theme = next;
    applyTheme(theme);
  }
  async function signOut() {
    if (signingOut) return;
    signingOut = true;
    try {
      if (!(await mayLeave('/login'))) return;
      await api.auth.logout();
      signedOut();
    } catch (error) {
      failed('Signing out failed', error);
    } finally {
      signingOut = false;
    }
  }

  function closeMenu(event: KeyboardEvent) {
    if (event.key !== 'Escape' || !menuOpen || event.defaultPrevented || document.querySelector('dialog[open]')) return;
    menuOpen = false;
    menuButton?.focus();
  }

  onMount(() => {
    applyTheme(theme);
    setSignedOutHandler(signedOut);
    void checkSession();
    return poll(() => session.state === 'unavailable' ? checkSession() : refreshPending(), 3000);
  });
  $effect(() => {
    const path = route.path;
    if (path === lastPath) return;
    lastPath = path;
    menuOpen = false;
    // Navigation must not leave keyboard focus on a now-hidden mobile link. A control
    // inside the page that is still there (a settings category) keeps it, though.
    void tick().then(() => {
      const active = document.activeElement;
      if (active && active !== content && content?.contains(active)) return;
      content?.focus({ preventScroll: true });
    });
  });
</script>

<svelte:window onkeydown={closeMenu} />

{#if session.state === 'checking'}
  <div class="loading" aria-busy="true"></div>
{:else if session.state === 'unavailable'}
  <div class="connection-error">
    <h1>Cannot connect to Rubylight</h1>
    <p role="alert">{session.error}</p>
    <p>The console retries automatically. Check that Rubylight is running on the host.</p>
    <Button onclick={checkSession}>Retry now</Button>
  </div>
{:else if bare}
  <Page />
{:else}
  <div class="shell">
    <a class="skip-link" href="#main-content" onclick={(event) => { event.preventDefault(); content?.focus(); }}>Skip to content</a>
    <aside class:open={menuOpen}>
      <div class="side">
        <div class="brand-row">
          <a class="brand" href="/" use:link>
            <span class="mark" aria-hidden="true">R</span>
            <span>Rubylight</span>
          </a>
          <button bind:this={menuButton} class="menu" aria-label="Menu" aria-controls="main-navigation" aria-expanded={menuOpen} onclick={() => (menuOpen = !menuOpen)}>
            <Icon name="menu" />
          </button>
        </div>
        <nav id="main-navigation" aria-label="Main">
          {#snippet links(items: NavItem[])}
            {#each items as item (item.path)}
              <a href={item.path} use:link aria-current={current(item) ? 'page' : undefined}>
                <Icon name={item.icon} />
                {item.label}
              </a>
            {/each}
          {/snippet}
          <div class="nav-group">{@render links(primary)}</div>
          <details class="nav-group advanced" open={advancedOpen || inAdvanced} ontoggle={(event) => (advancedOpen = event.currentTarget.open)}>
            <summary><Icon name="chevron" size={16} />Advanced</summary>
            <div class="nav-group">{@render links(advanced)}</div>
          </details>
        </nav>
        <div class="foot">
          {#if session.metadata}
            <div class="host">
              <span class="name">{session.metadata.host_name}</span>
              <span class="muted">Version <span class="num">{session.metadata.version}</span></span>
              <span class="address">
                <span class="num" title={session.metadata.pc_address}>{session.metadata.pc_address}</span>
                <CopyButton value={session.metadata.pc_address} label="Copy address" what="the address" />
              </span>
            </div>
          {/if}
          <div class="foot-actions">
            <div class="appearance"><span>Appearance</span><div class="theme" role="group" aria-label="Theme">
              {#each themes as option (option.value)}
                <button
                  type="button"
                  aria-pressed={theme === option.value}
                  aria-label="{option.label} theme"
                  title="Theme: {option.label}"
                  onclick={() => setTheme(option.value)}
                >
                  <Icon name={option.icon} size={16} />
                </button>
              {/each}
            </div></div>
            <button class="foot-button" disabled={signingOut} aria-busy={signingOut} onclick={signOut}>
              <Icon name="signout" size={16} />
              {signingOut ? 'Signing out…' : 'Sign out'}
            </button>
          </div>
        </div>
      </div>
    </aside>
    <main id="main-content" tabindex="-1" bind:this={content}>
      {#if session.pending.length && route.path !== '/devices'}
        <PairingBanner requests={session.pending} />
      {/if}
      <Page />
    </main>
  </div>
{/if}

<ConfirmHost />
<Toaster />

<style>
  .skip-link {
    position: fixed;
    z-index: 100;
    top: 8px;
    left: 8px;
    padding: 10px var(--space-4);
    background: var(--panel);
    border: 2px solid var(--focus);
    border-radius: var(--radius);
    transform: translateY(-150%);
  }
  .skip-link:focus {
    transform: translateY(0);
  }
  main:focus {
    outline: none;
  }
  .loading {
    min-height: 100vh;
  }
  .connection-error {
    display: grid;
    gap: var(--space-4);
    max-width: 600px;
    margin: 15vh auto;
    padding: var(--space-5);
  }
  .shell {
    display: grid;
    grid-template-columns: var(--nav-width) minmax(0, 1fr);
    min-height: 100vh;
    min-height: 100dvh;
  }
  /* The sidebar's background runs the full page height; its content stays in view. */
  aside {
    border-right: 1px solid var(--line);
    background: var(--bg);
  }
  .side {
    position: sticky;
    top: 0;
    height: 100vh;
    height: 100dvh;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
    padding: var(--space-5) var(--space-3) var(--space-4);
  }
  .brand-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0 var(--space-2);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-family: var(--font-display);
    font-weight: var(--weight-bold);
    font-size: 25px;
    text-decoration: none;
    letter-spacing: -0.01em;
  }
  .mark {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: var(--radius-sm);
    background: var(--accent);
    color: var(--accent-ink);
    font-weight: 800;
    font-size: 26px;
  }
  .menu {
    display: none;
    border: 0;
    background: none;
    padding: 10px;
    min-width: var(--control-height);
    min-height: var(--control-height);
    cursor: pointer;
  }
  nav {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-3);
  }
  .nav-group {
    display: grid;
    gap: var(--space-2);
  }
  .advanced {
    margin-top: auto;
    padding-top: var(--space-3);
    border-top: 1px solid var(--line);
  }
  .advanced summary {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: var(--control-height);
    padding: 0 14px;
    border-radius: var(--radius);
    color: var(--muted);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    cursor: pointer;
    list-style: none;
  }
  .advanced summary::-webkit-details-marker {
    display: none;
  }
  .advanced summary:hover {
    background: var(--sunken);
    color: var(--ink);
  }
  .advanced summary :global(svg) {
    transition: transform var(--duration-fast);
  }
  .advanced[open] summary :global(svg) {
    transform: rotate(90deg);
  }
  .advanced a {
    min-height: var(--control-height);
    padding: 10px 14px;
    font-size: var(--text-sm);
  }
  nav a {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 15px 14px;
    min-height: 52px;
    border-radius: var(--radius);
    color: var(--ink-2);
    text-decoration: none;
    font-weight: 520;
  }
  nav a:hover {
    background: var(--sunken);
    color: var(--ink);
  }
  nav a[aria-current='page'] {
    background: var(--accent-soft);
    color: var(--ink);

  }
  .foot {
    margin-top: auto;
    display: grid;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-2) 0;
    border-top: 1px solid var(--line);
  }
  .host {
    display: grid;
    gap: 1px;
    font-size: var(--text-sm);
    min-width: 0;
  }
  .host .name {
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }
  .host .muted {
    font-size: var(--text-xs);
  }
  .address {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    font-size: var(--text-xs);
    color: var(--ink-2);
  }
  /* One address, never broken across lines; the tooltip has it in full. */
  .address .num {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .foot-actions {
    display: grid;
    gap: var(--space-3);
  }
  .appearance {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    flex-wrap: wrap;
    font-size: var(--text-sm);
  }
  .theme {
    display: inline-flex;
    padding: 2px;
    gap: 1px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
  }
  .theme button {
    display: grid;
    place-items: center;
    width: 44px;
    height: var(--control-height);
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--muted);
    cursor: pointer;
  }
  .theme button:hover {
    color: var(--ink);
    background: var(--sunken);
  }
  .theme button[aria-pressed='true'] {
    background: var(--sunken);
    color: var(--ink);
    box-shadow: inset 0 0 0 1px var(--line-strong);
  }
  .foot-button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: none;
    padding: 10px var(--space-2);
    min-height: var(--control-height);
    font-size: var(--text-xs);
    color: var(--ink-2);
    cursor: pointer;
  }
  .foot-button:hover {
    background: var(--sunken);
  }
  main {
    padding: var(--space-6) var(--space-6) var(--space-7);
    max-width: 1500px;
    width: 100%;
    display: grid;
    align-content: start;
    gap: var(--space-5);
  }
  @media (max-width: 860px) {
    .shell {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: auto 1fr;
    }
    aside {
      position: sticky;
      top: 0;
      z-index: 10;
      border-right: 0;
      border-bottom: 1px solid var(--line);
    }
    .side {
      position: static;
      height: auto;
      max-height: 100vh;
      max-height: 100dvh;
      padding: var(--space-3);
      gap: var(--space-3);
    }
    .menu {
      display: block;
    }
    nav,
    .foot {
      display: none;
    }
    aside.open nav,
    aside.open .foot {
      display: grid;
    }
    nav a {
      min-height: var(--control-height);
    }
    .theme button {
      width: 44px;
      height: var(--control-height);
    }
    .foot-button {
      min-height: var(--control-height);
      padding: 6px 14px;
    }
    main {
      padding: var(--space-5) var(--space-4) var(--space-6);
    }
  }
</style>
