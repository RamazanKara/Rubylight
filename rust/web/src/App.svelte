<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Icon, { type IconName } from './components/Icon.svelte';
  import Button from './components/Button.svelte';
  import AuthLayout from './components/AuthLayout.svelte';
  import BrandMark from './components/BrandMark.svelte';
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
  import SettingsHome from './pages/SettingsHome.svelte';
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
    if (route.path === '/settings') return SettingsHome;
    if (match('/settings/:section')) return Settings;
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
  <AuthLayout>
    <h1>Cannot connect to Rubylight</h1>
    <p>The console retries automatically. Check that Rubylight is running on the host.</p>
    <p class="notice danger" role="alert">{session.error}</p>
    <Button variant="primary" onclick={checkSession}>Retry now</Button>
  </AuthLayout>
{:else if bare}
  <Page />
{:else}
  <div class="shell">
    <a class="skip-link" href="#main-content" onclick={(event) => { event.preventDefault(); content?.focus(); }}>Skip to content</a>
    <aside class:open={menuOpen}>
      <div class="side">
        <div class="brand-row">
          <a class="brand" href="/" use:link>
            <BrandMark size={32} />
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
              <span class="address">
                <span class="num" title={session.metadata.pc_address}>{session.metadata.pc_address}</span>
                <CopyButton value={session.metadata.pc_address} label="Copy address" what="the address" />
              </span>
              <span class="version">Version <span class="num">{session.metadata.version}</span></span>
            </div>
          {/if}
          <div class="theme" role="group" aria-label="Theme">
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
          </div>
          <button class="foot-button" disabled={signingOut} aria-busy={signingOut} onclick={signOut}>
            <Icon name="signout" size={16} />
            {signingOut ? 'Signing out…' : 'Sign out'}
          </button>
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
  .shell {
    display: grid;
    grid-template-columns: var(--nav-width) minmax(0, 1fr);
    min-height: 100vh;
    min-height: 100dvh;
  }
  /* The sidebar's background runs the full page height; its content stays in
     view. Only the links scroll on a short window: the brand and the footer
     (host, theme, sign out) stay reachable. */
  aside {
    border-right: 1px solid var(--line);
    background: var(--bg);
  }
  .side {
    position: sticky;
    top: 0;
    height: 100vh;
    height: 100dvh;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: var(--space-4) var(--space-3);
  }
  .brand-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    min-height: var(--control-height);
    padding: 0 var(--space-2);
  }
  .brand {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    min-height: var(--control-height);
    border-radius: var(--radius);
    font-family: var(--font-display);
    font-weight: var(--weight-bold);
    font-size: var(--text-lg);
    text-decoration: none;
    letter-spacing: -0.01em;
  }
  .menu {
    display: none;
    place-items: center;
    border: 0;
    border-radius: var(--radius);
    background: none;
    padding: 0;
    color: var(--ink-2);
    cursor: pointer;
  }
  .menu:hover {
    background: var(--sunken);
    color: var(--ink);
  }
  nav {
    display: flex;
    flex: 1 1 auto;
    min-height: 0;
    flex-direction: column;
    gap: var(--space-3);
    margin: 0 calc(var(--space-1) * -1);
    padding: 0 var(--space-1);
    overflow-y: auto;
    overscroll-behavior: contain;
  }
  .nav-group {
    display: grid;
    gap: 2px;
  }
  .advanced {
    margin-top: auto;
    padding-top: var(--space-2);
    border-top: 1px solid var(--line);
  }
  .advanced summary {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 0 var(--space-3);
    border-radius: var(--radius);
    color: var(--muted);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    transition: background var(--duration-fast), color var(--duration-fast);
  }
  .advanced summary:hover {
    background: var(--sunken);
    color: var(--ink);
  }
  .advanced summary :global(svg) {
    transition: transform var(--duration-fast) var(--ease-standard);
  }
  .advanced[open] summary :global(svg) {
    transform: rotate(90deg);
  }
  .advanced .nav-group {
    padding-top: 2px;
  }
  .advanced a {
    font-size: var(--text-sm);
  }
  nav a {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 0 var(--space-3);
    min-height: var(--control-height);
    border-radius: var(--radius);
    color: var(--ink-2);
    text-decoration: none;
    font-weight: var(--weight-medium);
    transition: background var(--duration-fast), color var(--duration-fast);
  }
  nav a :global(svg) {
    flex: none;
    color: var(--muted);
    transition: color var(--duration-fast);
  }
  nav a:hover {
    background: var(--sunken);
    color: var(--ink);
  }
  nav a:hover :global(svg) {
    color: var(--ink-2);
  }
  nav a[aria-current='page'] {
    background: var(--accent-soft);
    color: var(--ink);
    font-weight: var(--weight-semibold);
  }
  nav a[aria-current='page'] :global(svg) {
    color: var(--accent-text);
  }
  .foot {
    flex: none;
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3) 0 0;
    border-top: 1px solid var(--line);
  }
  .host {
    display: grid;
    min-width: 0;
    padding: 0 var(--space-3);
    font-size: var(--text-sm);
  }
  .host .name {
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }
  /* The copy button keeps its 44 px target but overlaps the lines around it. */
  .address {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    margin: -10px 0 -10px;
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
  .version {
    font-size: var(--text-xs);
    color: var(--muted);
  }
  .theme {
    display: grid;
    grid-template-columns: repeat(3, minmax(var(--control-height), 1fr));
    overflow: hidden;
    border: 1px solid var(--line);
    border-radius: var(--radius);
  }
  .theme button {
    display: grid;
    place-items: center;
    height: var(--control-height);
    padding: 0;
    border: 0;
    border-radius: 0;
    background: none;
    color: var(--muted);
    cursor: pointer;
    transition: background var(--duration-fast), color var(--duration-fast);
  }
  .theme button:hover {
    color: var(--ink);
    background: var(--sunken);
  }
  .theme button + button {
    border-left: 1px solid var(--line);
  }
  .theme button:focus-visible {
    outline-offset: -3px;
  }
  .theme button[aria-pressed='true'] {
    background: var(--accent-soft);
    color: var(--accent-text);
  }
  .foot-button {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    border: 0;
    border-radius: var(--radius);
    background: none;
    padding: 0 var(--space-3);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    color: var(--ink-2);
    cursor: pointer;
    transition: background var(--duration-fast), color var(--duration-fast);
  }
  .foot-button :global(svg) {
    color: var(--muted);
  }
  .foot-button:hover:not(:disabled) {
    background: var(--sunken);
    color: var(--ink);
  }
  .foot-button:disabled {
    cursor: default;
    opacity: 0.6;
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
      z-index: var(--z-sticky);
      border-right: 0;
      border-bottom: 1px solid var(--line);
    }
    .side {
      position: static;
      height: auto;
      max-height: 100vh;
      max-height: 100dvh;
      overflow-y: auto;
      padding: var(--space-2) var(--space-3);
      gap: var(--space-3);
    }
    .brand-row {
      padding-right: 0;
    }
    .menu {
      display: grid;
    }
    nav,
    .foot {
      display: none;
    }
    aside.open nav {
      display: flex;
      flex: none;
      overflow: visible;
    }
    aside.open .foot {
      display: grid;
      padding-bottom: var(--space-2);
    }
    .advanced {
      margin-top: 0;
    }
    main {
      padding: var(--space-5) var(--space-4) var(--space-6);
    }
  }
</style>
