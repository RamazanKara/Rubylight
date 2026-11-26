<script lang="ts">
  import { onMount, tick } from 'svelte';
  import Icon from './components/Icon.svelte';
  import Button from './components/Button.svelte';
  import AuthLayout from './components/AuthLayout.svelte';
  import BrandMark from './components/BrandMark.svelte';
  import CopyButton from './components/CopyButton.svelte';
  import ConfirmHost from './components/ConfirmHost.svelte';
  import Toaster from './components/Toaster.svelte';
  import PairingBanner from './components/PairingBanner.svelte';
  import QuickSwitcher from './components/QuickSwitcher.svelte';
  import LiveBadge from './components/shell/LiveBadge.svelte';
  import ThemeSwitch from './components/shell/ThemeSwitch.svelte';
  import { api, setSignedOutHandler } from './lib/api';
  import { failed } from './lib/feedback.svelte';
  import { poll } from './lib/format';
  import { advanced, isCurrent, primary, type NavItem } from './lib/navigation';
  import { link, match, mayLeave, route } from './lib/router.svelte';
  import { checkSession, heartbeat, session, signedOut } from './lib/session.svelte';
  import { liveBadge } from './lib/shell';
  import { applyTheme, storedTheme, type Theme } from './lib/theme';
  import Overview from './pages/Overview.svelte';
  import Library from './pages/Library.svelte';
  import LibraryImport from './pages/LibraryImport.svelte';
  import Stream from './pages/Stream.svelte';
  import Devices from './pages/Devices.svelte';
  import Settings from './pages/Settings.svelte';
  import SettingsHome from './pages/SettingsHome.svelte';
  import Logs from './pages/Logs.svelte';
  import Sessions from './pages/Sessions.svelte';
  import Clips from './pages/Clips.svelte';
  import Help from './pages/Help.svelte';
  import Troubleshoot from './pages/Troubleshoot.svelte';
  import Tokens from './pages/Tokens.svelte';
  import Webhooks from './pages/Webhooks.svelte';
  import Login from './pages/Login.svelte';
  import Setup from './pages/Setup.svelte';
  import NotFound from './pages/NotFound.svelte';

  // The sidebar: the everyday pages, Help last; everything else in Advanced, folded (lib/navigation.ts).
  let theme = $state<Theme>(storedTheme());
  let menuOpen = $state(false);
  let signingOut = $state(false);
  let menuButton = $state<HTMLButtonElement>();
  let content = $state<HTMLElement>();
  let lastPath = route.path;
  /** The quick switcher (Ctrl+K or ⌘K). */
  let switcherOpen = $state(false);
  /** Bumped to draw the page again after the switcher synced the games the Games page lists. */
  let pageKey = $state(0);
  const mac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent);
  /** A phone or a narrow window: the rail becomes a top bar and takes Search and the theme into its menu. */
  const narrow = typeof matchMedia === 'function' ? matchMedia('(max-width: 860px)') : null;
  let compact = $state(narrow?.matches ?? false);
  const badge = $derived(liveBadge(session.live));

  const current = (item: NavItem) => isCurrent(item, route.path);
  const inAdvanced = $derived(advanced.some(current));
  let advancedOpen = $state(false);

  const Page = $derived.by(() => {
    if (session.state === 'needs-setup' || route.path === '/setup') return Setup;
    if (session.state === 'signed-out' || route.path === '/login') return Login;
    if (route.path === '/') return Overview;
    if (route.path === '/library/import') return LibraryImport;
    if (match('/library') || match('/library/:uuid')) return Library;
    if (route.path === '/stream') return Stream;
    if (route.path === '/devices') return Devices;
    if (route.path === '/settings') return SettingsHome;
    if (match('/settings/:section')) return Settings;
    if (route.path === '/logs') return Logs;
    if (route.path === '/help') return Help;
    if (route.path === '/troubleshoot') return Troubleshoot;
    if (route.path === '/sessions') return Sessions;
    if (route.path === '/clips') return Clips;
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

  /** Ctrl+K, or ⌘K on a Mac, opens the quick switcher anywhere in the console and closes it again. */
  function shortcut(event: KeyboardEvent) {
    if (event.defaultPrevented || event.altKey || event.shiftKey || !(event.ctrlKey || event.metaKey)) return;
    if (event.code !== 'KeyK' && event.key.toLowerCase() !== 'k') return;
    // A confirmation keeps the keyboard until it is answered.
    if (!switcherOpen && document.querySelector('dialog[open]')) return;
    event.preventDefault();
    if (!switcherOpen) menuOpen = false;
    switcherOpen = !switcherOpen;
  }

  function openSwitcher() {
    menuOpen = false;
    switcherOpen = true;
  }

  onMount(() => {
    applyTheme(theme);
    setSignedOutHandler(signedOut);
    void checkSession();
    return poll(heartbeat, 3000);
  });
  onMount(() => {
    const resized = () => (compact = narrow?.matches ?? false);
    narrow?.addEventListener('change', resized);
    return () => narrow?.removeEventListener('change', resized);
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

<svelte:window
  onkeydown={(event) => {
    closeMenu(event);
    if (!bare && session.state === 'signed-in' && !session.reconnecting) shortcut(event);
  }}
/>

{#snippet cannotConnect()}
  <AuthLayout>
    <h1>Cannot connect to Rubylight</h1>
    <p>The console retries automatically. Check that Rubylight is running on the host.</p>
    <p class="notice danger" role="alert">{session.error}</p>
    <Button variant="primary" onclick={checkSession}>Retry now</Button>
  </AuthLayout>
{/snippet}

{#snippet search()}
  <button
    type="button"
    class="find"
    aria-label="Search"
    aria-haspopup="dialog"
    aria-keyshortcuts="Control+K Meta+K"
    title="Search ({mac ? '⌘K' : 'Ctrl+K'})"
    onclick={openSwitcher}
  >
    <Icon name="search" size={compact ? 20 : 17} />
    <span class="find-label">Search games, devices and settings</span>
    <kbd aria-hidden="true">{mac ? '⌘K' : 'Ctrl K'}</kbd>
  </button>
{/snippet}

{#if session.state === 'checking'}
  <div class="loading" aria-busy="true"></div>
{:else if session.state === 'unavailable'}
  {@render cannotConnect()}
{:else if bare}
  <Page />
{:else}
  <!-- The host stopped answering: the notice covers the page, which keeps its unsaved edits until the host is back. -->
  {#if session.reconnecting}<div class="reconnecting">{@render cannotConnect()}</div>{/if}
  <div class="shell" inert={session.reconnecting} aria-hidden={session.reconnecting || undefined}>
    <a class="skip-link" href="#main-content" onclick={(event) => { event.preventDefault(); content?.focus(); }}>Skip to content</a>
    <aside class:open={menuOpen}>
      <div class="side">
        <div class="brand-row">
          <a class="brand" href="/" use:link>
            <BrandMark size={32} />
            <span>Rubylight</span>
          </a>
          {#if compact}{@render search()}{/if}
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
                {#if item.path === '/' && compact && badge?.streaming}<LiveBadge {badge} dot />{/if}
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
              <span class="name"><Icon name="monitor" size={16} />{session.metadata.host_name}</span>
              <span class="address">
                <span class="num" title={session.metadata.pc_address}>{session.metadata.pc_address}</span>
                <CopyButton value={session.metadata.pc_address} label="Copy address" what="the address" />
              </span>
              <span class="version">Version <span class="num">{session.metadata.version}</span></span>
            </div>
          {/if}
          {#if compact}<ThemeSwitch {theme} onchange={setTheme} />{/if}
          <button class="foot-button" disabled={signingOut} aria-busy={signingOut} onclick={signOut}>
            <Icon name="signout" size={16} />
            {signingOut ? 'Signing out…' : 'Sign out'}
          </button>
        </div>
      </div>
    </aside>
    <div class="stage">
      {#if !compact}
        <!-- The command bar: find anything, see what plays now, pick the theme. -->
        <div class="bar" role="toolbar" aria-label="Console">
          {@render search()}
          <span class="bar-end">
            {#if badge}<LiveBadge {badge} />{/if}
            <ThemeSwitch {theme} onchange={setTheme} />
          </span>
        </div>
      {/if}
      <main id="main-content" tabindex="-1" bind:this={content}>
        {#if session.pending.length && route.path !== '/devices'}
          <PairingBanner requests={session.pending} />
        {/if}
        {#key pageKey}<Page />{/key}
      </main>
    </div>
  </div>
  <QuickSwitcher bind:open={switcherOpen} {theme} ontheme={setTheme} onlibrarychanged={() => route.path === '/library' && pageKey++} />
{/if}

<ConfirmHost />
<Toaster />

<style>
  .skip-link {
    position: fixed;
    z-index: var(--z-skip);
    inset-block-start: var(--space-2);
    inset-inline-start: var(--space-2);
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
  .reconnecting {
    position: fixed;
    inset: 0;
    z-index: var(--z-overlay);
    overflow-y: auto;
    background: var(--bg);
  }
  .shell {
    display: grid;
    grid-template-columns: var(--nav-width) minmax(0, 1fr);
    min-height: 100vh;
    min-height: 100dvh;
  }
  /* The rail's background runs the full page height; its content stays in view.
     Only the links scroll on a short window: the brand and the footer (host,
     sign out) stay reachable. */
  aside {
    border-inline-end: 1px solid var(--line);
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
    padding: var(--space-3) var(--space-3) var(--space-4);
  }
  .brand-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--space-1);
    min-height: calc(var(--bar-height) - var(--space-3));
    padding-inline: var(--space-2) 0;
  }
  .brand {
    display: inline-flex;
    align-items: center;
    gap: var(--space-3);
    min-height: var(--control-height);
    margin-inline-end: auto;
    border-radius: var(--radius);
    font-family: var(--font-display);
    font-weight: var(--weight-bold);
    font-size: var(--text-lg);
    text-decoration: none;
    letter-spacing: var(--tracking-display);
  }
  /* Search: a field-like button at the head of the command bar. It opens the quick switcher. */
  .find {
    flex: 0 1 460px;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-width: 0;
    padding-inline: var(--space-4) var(--space-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-pill);
    background: var(--panel);
    box-shadow: var(--edge);
    color: var(--muted);
    font-size: var(--text-sm);
    text-align: start;
    cursor: pointer;
    transition:
      background var(--duration-fast),
      color var(--duration-fast),
      border-color var(--duration-fast);
  }
  .find:hover {
    border-color: var(--line-strong);
    color: var(--ink);
  }
  .find-label {
    flex: 1 1 auto;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .find kbd {
    flex: none;
    padding: 2px var(--space-2);
    border: 1px solid var(--line);
    border-radius: var(--radius-pill);
    background: var(--sunken);
    color: var(--ink-2);
    font-size: var(--text-xs);
    line-height: var(--leading-snug);
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
    margin-inline: calc(var(--space-1) * -1);
    padding-inline: var(--space-1);
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
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
    transition:
      background var(--duration-fast),
      color var(--duration-fast);
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
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 0 var(--space-3);
    min-height: var(--control-height);
    border-radius: var(--radius);
    color: var(--ink-2);
    text-decoration: none;
    font-weight: var(--weight-medium);
    transition:
      background var(--duration-fast),
      color var(--duration-fast);
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
  /* The page shown: a tinted row with a ruby marker at its leading edge. */
  nav a[aria-current='page'] {
    background: var(--accent-soft);
    color: var(--ink);
    font-weight: var(--weight-semibold);
  }
  nav a[aria-current='page']::before {
    content: '';
    position: absolute;
    inset-inline-start: 0;
    inset-block: var(--space-3);
    width: 3px;
    border-radius: var(--radius-pill);
    background: var(--accent);
  }
  nav a[aria-current='page'] :global(svg) {
    color: var(--accent-text);
  }
  .foot {
    flex: none;
    display: grid;
    gap: var(--space-2);
  }
  /* This PC, as a small card: its name, the address a device connects to, and the version. */
  .host {
    display: grid;
    min-width: 0;
    padding: var(--space-3);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
    box-shadow: var(--edge);
    font-size: var(--text-sm);
  }
  .host .name {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }
  .host .name :global(svg) {
    flex: none;
    color: var(--muted);
  }
  /* The copy button keeps its 44 px target but overlaps the lines around it. */
  .address {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    margin-block: -8px;
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
    transition:
      background var(--duration-fast),
      color var(--duration-fast);
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
    color: var(--muted);
  }
  /* The page column: the command bar, then the page. */
  .stage {
    display: grid;
    grid-template-rows: auto 1fr;
    align-content: start;
    min-width: 0;
    width: 100%;
    max-width: 1500px;
    padding-inline: var(--space-6);
  }
  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    min-height: var(--bar-height);
    padding-block: var(--space-3) 0;
  }
  .bar-end {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-width: 0;
  }
  main {
    padding-block: var(--space-5) var(--space-7);
    min-width: 0;
    display: grid;
    align-content: start;
    gap: var(--space-5);
    isolation: isolate;
  }
  /* Page changes (lib/router.svelte.ts starts them; none under reduced motion): the rail
     switches at once and stays on top, the window behind it fades up, and a game's cover morphs
     into its page and back. Only elements no taller than the window carry a name: Firefox
     snapshots a taller one (the page, the full-height rail) as a cut-off slice that shrinks
     and then jumps back to full size. */
  aside {
    isolation: isolate;
  }
  .side {
    view-transition-name: shell;
  }
  :global(::view-transition-old(shell)) {
    display: none;
  }
  :global(::view-transition-new(shell)),
  :global(::view-transition-group(root)) {
    animation: none;
  }
  :global(::view-transition-group(shell)) {
    z-index: var(--z-base);
  }
  :global(::view-transition-old(root)) {
    animation: var(--duration-fast) var(--ease-exit) both rubylight-page-out;
    mix-blend-mode: normal;
  }
  :global(::view-transition-new(root)) {
    animation: var(--duration-slow) var(--ease-enter) both rubylight-page-in;
    mix-blend-mode: normal;
  }
  :global(::view-transition-group(game-cover)) {
    animation-duration: var(--duration-slow);
    animation-timing-function: var(--ease-standard);
  }
  @keyframes -global-rubylight-page-out {
    to {
      opacity: 0;
    }
  }
  @keyframes -global-rubylight-page-in {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
  }
  @media (max-width: 860px) {
    .shell {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: auto 1fr;
    }
    /* The phone's top bar is no taller than the window, so it carries the name itself. */
    aside {
      position: sticky;
      top: 0;
      z-index: var(--z-sticky);
      border-inline-end: 0;
      border-bottom: 1px solid var(--line);
      view-transition-name: shell;
    }
    .side {
      view-transition-name: none;
      position: static;
      height: auto;
      max-height: 100vh;
      max-height: 100dvh;
      overflow-y: auto;
      padding: var(--space-2) var(--space-3);
      gap: var(--space-3);
    }
    .brand-row {
      min-height: var(--control-height);
    }
    /* In the phone's top bar Search is an icon beside the menu, always in reach. */
    .find {
      flex: none;
      justify-content: center;
      min-width: var(--control-height);
      padding: 0;
      border-color: transparent;
      border-radius: var(--radius);
      background: none;
      box-shadow: none;
      color: var(--ink-2);
    }
    .find:hover {
      border-color: transparent;
      background: var(--sunken);
    }
    .find-label,
    .find kbd {
      display: none;
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
    .stage {
      padding-inline: var(--space-4);
    }
    main {
      padding-block: var(--space-5) var(--space-6);
    }
  }
</style>
