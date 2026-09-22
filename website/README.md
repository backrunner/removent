# Removent website

The public Removent site, built with **svedocs 0.2.2**, Svelte 5, and SvelteKit. It includes a custom landing page and documentation theme, English and Simplified Chinese content, local search, and system/light/dark appearance. There is no support page or support backend.

## Develop

Use Node 24+ and pnpm 12.3.4 (declared in `package.json`). From this directory:

```sh
pnpm install
pnpm dev
```

## Validate and preview

```sh
pnpm check
pnpm check:docs
pnpm build
pnpm exec playwright install chromium webkit
pnpm test
pnpm preview
```

The browser tests run against the production build. They cover protocol preview controls, localized navigation and search, theme persistence, 320–768px layouts, document anchors and code copy, metadata and markdown endpoints, downloads, and unknown routes. Screenshots and failure traces are written to ignored `test-results/`.

## Static hosting

`pnpm build` writes the complete site to `build/`. Deploy that directory to a static host that supports directory indexes and serves `404.html` with a 404 status for unknown paths. No Worker, database, AI provider, or API credentials are required.

The main GitHub Actions CI builds the site, validates both languages, runs Chromium and WebKit tests, and uploads the static output as the `Removent-website` artifact. The release workflow requires this CI to pass for the exact release commit. Hosting deployment is separate.

The default canonical origin is `https://removent.alkinum.com`. This is a configurable publishing target, not a claim that the domain has been deployed. Set the actual origin at build time:

```sh
SITE_URL=https://your-domain.example pnpm build
```

Use the same origin when generating and serving the site. Both `/` and `/zh` are prerendered, documentation lives at `/docs` and `/docs/zh`, and standalone pages include `/download`, `/privacy`, and their Chinese equivalents. The build also includes Open Graph images, sitemap, robots.txt, markdown twins, and the `llms.txt` interface. There are no runtime search endpoints: the search index loads in the browser on demand.

Download links intentionally point to the repository's Releases page so unpublished versions and changing asset filenames are never advertised as downloadable files.

## Theme and content

- `svedocs.config.ts`: site settings, origin, search, locales, metadata, and theme tokens.
- `vite.config.ts`: registered Navbar, DocsShell, and Footer replacements.
- `src/lib/theme.css`: Quiet Control colors, typography, responsive geometry, and custom reading styles.
- `src/lib/Landing.svelte`: product landing sections, real app screenshots, and relay diagram.
- `src/lib/ConnectionPreview.svelte`: keyboard-accessible protocol illustration; it does not open real sessions.
- `src/lib/messages.ts` and `shell-messages.ts`: localized interface copy.
- `content/docs/{en,zh}`: 11 guides per language, adapted from the current repository documentation.
- `content/pages/{en,zh}`: homepage metadata, download information, and privacy information.

Mirror relative Markdown paths between languages. Keep compiled content, page discovery, route resolution, search loading, metadata, and locale mapping owned by svedocs. `pnpm check:docs` validates links, assets, and complete translation coverage.

Brand assets come from `../assets/branding/`. Product screenshots come from `../docs/images/settings-native-{light,dark}.png`. The connection preview contains illustrative hostnames and no fabricated latency or connection measurements. Feature claims distinguish native RVP, VNC, and RDP; update the public guides when those implementations change.
