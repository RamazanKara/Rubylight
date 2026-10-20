import { tick } from 'svelte';

// History routing for the console's handful of pages.

export const route = $state({ path: location.pathname, search: location.search });

/** Asked before leaving the page in the app (e.g. unsaved changes). */
type Guard = (to: string) => boolean | Promise<boolean>;
let guard: Guard | null = null;
let index = typeof history === 'undefined' ? 0 : (history.state?.rubylightIndex ?? 0);
if (typeof history !== 'undefined') history.replaceState({ ...history.state, rubylightIndex: index }, '');
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
  sync();
});

function go(to: string, replace: boolean) {
  if (replace) history.replaceState({ ...history.state, rubylightIndex: index }, '', to);
  else history.pushState({ rubylightIndex: ++index }, '', to);
  sync();
  window.scrollTo(0, 0);
  if (location.hash) void tick().then(() => document.getElementById(location.hash.slice(1))?.scrollIntoView());
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
