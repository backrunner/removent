<script lang="ts">
  import { onMount } from 'svelte';
  import { useSvedocsTheme } from 'svedocs/theme/headless';
  import Icon from './Icon.svelte';

  const RELEASES_URL = 'https://github.com/backrunner/removent/releases';
  const API_URL = 'https://api.github.com/repos/backrunner/removent/releases/latest';
  const DMG_PATTERN = /^Removent-[\d.]+-macos-arm64\.dmg$/;

  type ReleaseAsset = { name: string; size: number; browser_download_url: string };
  type ReleaseState = 'loading' | 'ready' | 'missing' | 'error';

  const context = useSvedocsTheme();
  $: t = $context.t;

  let state: ReleaseState = 'loading';
  let dmg: ReleaseAsset | undefined;
  let version = '';
  let notesUrl = RELEASES_URL;

  onMount(async () => {
    try {
      const response = await fetch(API_URL, { headers: { Accept: 'application/vnd.github+json' } });
      if (!response.ok) throw new Error(`GitHub releases returned ${response.status}`);
      const data = await response.json();
      const assets: ReleaseAsset[] = Array.isArray(data.assets) ? data.assets : [];
      dmg = assets.find((asset) => DMG_PATTERN.test(asset.name));
      version = String(data.tag_name ?? '').replace(/^v/, '');
      if (typeof data.html_url === 'string' && data.html_url) notesUrl = data.html_url;
      state = dmg ? 'ready' : 'missing';
    } catch {
      state = 'error';
    }
  });

  function formatSize(bytes: number): string {
    if (!Number.isFinite(bytes) || bytes <= 0) return '';
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
</script>

<div class="rv-release">
  {#if state === 'ready' && dmg}
    <a class="rv-button rv-primary" href={dmg.browser_download_url}>
      <Icon name="down" size={18} />{t('download.ctaVersion', { version })}
    </a>
    <p class="rv-release-meta">
      <code>{dmg.name}</code><span aria-hidden="true"> · </span>{formatSize(dmg.size)}<span aria-hidden="true"> · </span><a href={notesUrl}>{t('download.notes')}</a><span aria-hidden="true"> · </span><a href={RELEASES_URL}>{t('download.all')}</a>
    </p>
  {:else}
    <a class="rv-button rv-primary" href={state === 'missing' ? notesUrl : RELEASES_URL}>
      <Icon name="down" size={18} />{t('download.cta')}
    </a>
    {#if state === 'missing'}
      <p class="rv-release-meta">{t('download.missing')} <a href={RELEASES_URL}>{t('download.all')}</a></p>
    {:else if state === 'error'}
      <p class="rv-release-meta">{t('download.error')} <a href={RELEASES_URL}>{t('download.all')}</a></p>
    {/if}
  {/if}
</div>

<style>
  .rv-release { margin: 24px 0 8px; }
  .rv-release .rv-button { text-decoration: none; }
  .rv-release-meta { margin: 14px 0 0; font-size: 12px; color: var(--rv-muted); }
  .rv-release-meta code { font-family: var(--font-mono); font-size: 11px; }
  .rv-release-meta a { color: var(--rv-blue); font-weight: 540; }
  .rv-release-meta a:hover { text-decoration: underline; text-underline-offset: 4px; }
</style>
