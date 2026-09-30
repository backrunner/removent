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

The browser tests run against the production build. They cover protocol preview controls, localized navigation and search, theme persistence, 320–768px layouts, grouped documentation navigation, mobile page contents, document anchors and code copy, metadata and markdown endpoints, download fallbacks, and unknown routes. Screenshots and failure traces are written to ignored `test-results/`.

## Deploy to Cloudflare

The production site is **https://removent.pwp.sh**. `pnpm build` writes the complete site to `build/`; Wrangler deploys it as Cloudflare Workers Static Assets using `wrangler.jsonc`. Cloudflare manages the custom domain and HTTPS certificate. Unknown paths serve `404.html` with a 404 status. No runtime Worker code, database, AI provider, or application API credentials are required.

With Wrangler authenticated to the Alkinum account, deploy from this directory:

```sh
pnpm install --frozen-lockfile
pnpm run deploy
```

The deploy command checks types and documentation, builds the site, and runs `wrangler deploy`. To inspect the deployment configuration without publishing, run `pnpm build` followed by `pnpm exec wrangler deploy --dry-run`.

The main GitHub Actions CI builds the site, validates both languages, runs Chromium and WebKit tests, and uploads the static output as the `Removent-website` artifact. The release workflow requires this CI to pass for the exact release commit. Website deployment uses the Wrangler command above and does not run automatically on an app release.

The default canonical origin is `https://removent.pwp.sh`. Override it only when building for a different publishing target, and update the Wrangler custom domain to match:

```sh
SITE_URL=https://your-domain.example pnpm build
```

Use the same origin when generating and serving the site. Both `/` and `/zh` are prerendered, documentation lives at `/docs` and `/docs/zh`, and standalone pages include `/download`, `/privacy`, and their Chinese equivalents. The build also includes Open Graph images, sitemap, robots.txt, markdown twins, and the `llms.txt` interface. There are no runtime search endpoints: the search index loads in the browser on demand.

The main download button resolves the latest stable DMG through GitHub's release API, with a Releases-page fallback when lookup fails. Beta announcements in the homepage, docs sidebar, and download pages use separate, explicit release-notes links. Keep all three surfaces in sync when publishing the next beta. Publish the referenced beta before deploying its download-page announcement; prereleases must not replace the stable button or automatic-update feed.

## Theme and content

- `svedocs.config.ts`: site settings, origin, search, locales, metadata, and theme tokens.
- `vite.config.ts`: registered Navbar, DocsShell, and Footer replacements.
- `src/lib/theme.css`: slate, blue, and teal theme tokens, typography, responsive geometry, and reading surfaces.
- `src/lib/theme/DocsShell.svelte` and `DocsNavigation.svelte`: grouped guide navigation, overview shortcuts, reading panel, mobile contents, and source links.
- `src/lib/Landing.svelte`: asymmetric product hero, protocol preview, real app screenshots, and relay diagram.
- `src/lib/ReleaseDownload.svelte`: stable-release download card with a bounded GitHub lookup and fallback.
- `src/lib/ConnectionPreview.svelte`: keyboard-accessible protocol illustration; it does not open real sessions.
- `src/lib/messages.ts` and `shell-messages.ts`: localized interface copy.
- `content/docs/{en,zh}`: 11 guides per language, adapted from the current repository documentation.
- `content/pages/{en,zh}`: homepage metadata, download information, and privacy information.

Mirror relative Markdown paths between languages. Keep compiled content, page discovery, route resolution, search loading, metadata, and locale mapping owned by svedocs. `pnpm check:docs` validates links, assets, and complete translation coverage.

Brand assets come from `../assets/branding/`. Product screenshots come from `../docs/images/settings-native-{light,dark}.png`. The connection preview contains illustrative hostnames and no fabricated latency or connection measurements. Feature claims distinguish native RVP, VNC, and RDP; update the public guides when those implementations change.
