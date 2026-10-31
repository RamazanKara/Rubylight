<script lang="ts">
  // Rubylight's mark: the same white "R" on a rounded ruby square as the app
  // icon (rust/assets/make-icon.ps1) and the favicon, so the console, the tray
  // and setup all show one mark. Decorative: the name always sits beside it.
  let { size = 32 }: { size?: number } = $props();
</script>

<svg class="brand-mark" width={size} height={size} viewBox="0 0 32 32" aria-hidden="true" focusable="false">
  <rect width="32" height="32" rx="8" fill="var(--brand)" />
  <text x="16" y="23.5" text-anchor="middle" fill="var(--brand-ink)">R</text>
</svg>

<style>
  .brand-mark {
    flex: none;
    display: block;
  }
  text {
    font-family: 'Segoe UI', var(--font);
    font-size: 22px;
    font-weight: 700;
  }
</style>
