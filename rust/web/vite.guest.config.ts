import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// The guest page (docs/browser-guests.md): its own build, so it carries nothing of the console.
// `npm run build` runs it after the console's, into dist/guest, which the host serves under /guest/.
const web = fileURLToPath(new URL('.', import.meta.url));

export default defineConfig({
  root: fileURLToPath(new URL('./guest', import.meta.url)),
  base: '/guest/',
  plugins: [svelte({ configFile: `${web}svelte.config.js` })],
  build: {
    outDir: fileURLToPath(new URL('./dist/guest', import.meta.url)),
    emptyOutDir: true,
    assetsDir: 'assets',
    sourcemap: false,
    target: 'es2022',
  },
});
