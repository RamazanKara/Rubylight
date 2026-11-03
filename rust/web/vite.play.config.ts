import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// The play page (docs/webrtc.md, "Play in a TV browser"): its own build, so it carries nothing of
// the console. `npm run build` runs it after the guest page's, into dist/play, which the host
// serves under /play/ on the guest port.
const web = fileURLToPath(new URL('.', import.meta.url));

export default defineConfig({
  root: fileURLToPath(new URL('./play', import.meta.url)),
  base: '/play/',
  plugins: [svelte({ configFile: `${web}svelte.config.js` })],
  build: {
    outDir: fileURLToPath(new URL('./dist/play', import.meta.url)),
    emptyOutDir: true,
    assetsDir: 'assets',
    sourcemap: false,
    target: 'es2022',
  },
});
