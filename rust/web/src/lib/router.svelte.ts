import { tick } from 'svelte';

// History routing for the console's handful of pages.

/**
 * Pages that moved, by their old path. A link or bookmark to one opens the page that took its place,
 * and the address bar shows the new path. /maintenance was the page called Help.
 */
export const MOVED: Readonly<Record<string, string>> = { '/maintenance': '/help' };

/** A console address with a moved page's path replaced; any other address as it is. */
export function canonical(to: string): string {
  const end = to.search(/[?#]/);
  const path = end === -1 ? to : to.slice(0, end);
  const moved = MOVED[path];
  return moved ? moved + (end === -1 ? '' : to.slice(end)) : to;
}

let index = typeof history === 'undefined' ? 0 : (history.state?.rubylightIndex ?? 0);
if (typeof history !== 'undefined') {
  history.replaceState({ ...history.state, rubylightIndex: index }, '', canonical(location.pathname + location.search + location.hash));
}

export const route = $state({ path: location.pathname, search: location.search });

/** Asked before leaving the page in the app (e.g. unsaved changes). */
type Guard = (to: string) => boolean | Promise<boolean>;
let guard: Guard | null = null;
let restoring = false;
let deciding = false;

/** Install a guard; returns a function that removes it. */
export function guardNavigation(check: Guard): () => void {
  guard = check;
  return () => {
    if (guard === check) guard = null;
  };
}

function sync() {
  const moved = canonical(location.pathname);
  if (moved !== location.pathname) history.replaceState(history.state, '', moved + location.search + location.hash);
  route.path = location.pathname;
  route.search = location.search;
}
window.addEventListener('popstate', async (event) => {
  const next = (event as PopStateEvent).state?.rubylightIndex ?? 0;
  if (restoring) {
    restoring = false;
    return;
  }
  if (guard) {
    if (deciding) return;
    deciding = true;
    const leave = await guard(location.pathname + location.search);
    deciding = false;
    if (!leave) {
      const current = history.state?.rubylightIndex ?? 0;
      if (index !== current) {
        restoring = true;
        history.go(index - current);
      }
      return;
    }
  }
  index = history.state?.rubylightIndex ?? next;
  // Back and forward swap the page at once, with no transition: the browser restores the
  // scroll position as the entry changes, which a page drawn a frame later would lose.
  sync();
});

/** The name a game's cover carries while it morphs into the game's page or back (App.svelte styles it). */
export const MORPH = 'game-cover';

/** The game whose page a path is, or null for every other page. */
export function gamePage(path: string): string | null {
  const uuid = match('/library/:uuid', path)?.uuid;
  return uuid && uuid !== 'new' && uuid !== 'import' ? uuid : null;
}

type Swap = { ready: Promise<void>; finished: Promise<void>; updateCallbackDone: Promise<void> };
type Transitions = { startViewTransition?: (update: () => Promise<void>) => Swap };

/** The first cover of this game in the window (AppCover's `data-cover`); one off-screen would fly in from outside. */
function coverInView(uuid: string): HTMLElement | null {
  for (const cover of document.querySelectorAll<HTMLElement>(`[data-cover="${CSS.escape(uuid)}"]`)) {
    const box = cover.getBoundingClientRect();
    if (box.width > 0 && box.bottom > 0 && box.right > 0 && box.top < innerHeight && box.left < innerWidth) return cover;
  }
  return null;
}

/**
 * Draws the next page through a View Transition: the page fades up, and a game's cover morphs into its
 * page and back. Where the browser has none, or motion is reduced, `update` runs at once as before. A
 * transition the browser skips still runs `update`, so navigation never depends on it.
 */
function transition(from: string, to: string, update: () => void) {
  // Typed here: a browser without them has no startViewTransition at all.
  const start = (document as unknown as Transitions).startViewTransition;
  if (from === to || !start || matchMedia('(prefers-reduced-motion: reduce)').matches) {
    update();
    return;
  }
  const game = gamePage(to) ?? gamePage(from);
  // A name belongs to one element per snapshot: the cover carries it for its own page only.
  let named: HTMLElement | null = null;
  const name = () => {
    named = game ? coverInView(game) : null;
    if (named) named.style.viewTransitionName = MORPH;
  };
  const unname = () => {
    if (named) named.style.viewTransitionName = '';
    named = null;
  };
  name();
  const swap = start.call(document, async () => {
    unname();
    update();
    await tick();
    name();
  });
  swap.ready.catch(() => {});
  swap.updateCallbackDone.catch(() => {});
  swap.finished.then(unname, unname);
}

function go(to: string, replace: boolean) {
  const from = location.pathname;
  if (replace) history.replaceState({ ...history.state, rubylightIndex: index }, '', to);
  else history.pushState({ rubylightIndex: ++index }, '', to);
  const show = () => {
    sync();
    window.scrollTo(0, 0);
    if (location.hash) void tick().then(() => document.getElementById(location.hash.slice(1))?.scrollIntoView());
  };
  // A replaced entry is the page settling (a redirect, a new app saved): it gets no motion.
  if (replace) show();
  else transition(from, location.pathname, show);
}

/** Check the current draft before an action that leaves the page, such as signing out. */
export async function mayLeave(to: string): Promise<boolean> {
  if (deciding) return false;
  if (!guard) return true;
  deciding = true;
  try {
    return await guard(to);
  } finally {
    deciding = false;
  }
}

export function navigate(to: string, options: { replace?: boolean; force?: boolean } = {}) {
  to = canonical(to);
  if (to === location.pathname + location.search) return;
  if (!guard || options.force) {
    go(to, options.replace ?? false);
    return;
  }
  void mayLeave(to).then((leave) => {
    if (leave) go(to, options.replace ?? false);
  });
}

/** Match `/library/:uuid` style patterns; returns the parameters or null. */
export function match(pattern: string, path = route.path): Record<string, string> | null {
  const want = pattern.split('/').filter(Boolean);
  const have = path.split('/').filter(Boolean);
  if (want.length !== have.length) return null;
  const params: Record<string, string> = {};
  for (const [index, part] of want.entries()) {
    const value = have[index] ?? '';
    if (part.startsWith(':')) {
      try {
        params[part.slice(1)] = decodeURIComponent(value);
      } catch {
        return null;
      }
    }
    else if (part !== value) return null;
  }
  return params;
}

export function query(name: string): string | null {
  return new URLSearchParams(route.search).get(name);
}

/** Use on <a href> elements: same-origin clicks stay in the page. */
export function link(node: HTMLAnchorElement) {
  const onclick = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const url = new URL(node.href, location.href);
    if (url.origin !== location.origin || node.target || url.pathname.startsWith('/api/')) return;
    event.preventDefault();
    navigate(url.pathname + url.search + url.hash);
  };
  node.addEventListener('click', onclick);
  return { destroy: () => node.removeEventListener('click', onclick) };
}
