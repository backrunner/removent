import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import { svedocs } from 'svedocs/vite';
import config from './svedocs.config.ts';
export default defineConfig({
  plugins: [
    svedocs({ config, components: {
      ReleaseDownload: '$lib/ReleaseDownload.svelte'
    }, theme: { components: {
      Navbar: '$lib/theme/Navbar.svelte',
      DocsShell: '$lib/theme/DocsShell.svelte',
      Footer: '$lib/theme/Footer.svelte'
    } } }),
    tailwindcss(), sveltekit()
  ]
});
