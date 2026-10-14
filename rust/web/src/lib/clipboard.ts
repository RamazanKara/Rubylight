// Copying text, also where the async clipboard API is missing (a page that is
// not a secure context, or a browser that blocks it).

export async function copyText(text: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // Permission denied or no focus: try the selection below.
    }
  }
  const area = document.createElement('textarea');
  area.value = text;
  area.setAttribute('readonly', '');
  area.style.position = 'fixed';
  area.style.opacity = '0';
  document.body.append(area);
  const focused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  area.select();
  const copied = document.execCommand('copy');
  area.remove();
  focused?.focus({ preventScroll: true });
  if (!copied) throw new Error('the browser did not allow copying');
}
