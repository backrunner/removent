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

  onMount(() => {
    const controller = new AbortController();
    let disposed = false;
    const timeout = setTimeout(() => controller.abort(), 10000);
    async function loadRelease() {
      try {
        const response = await fetch(API_URL, { headers: { Accept: 'application/vnd.github+json' }, signal: controller.signal });
        if (!response.ok) throw new Error(`GitHub releases returned ${response.status}`);
        const data = await response.json();
        if (disposed) return;
        const assets: ReleaseAsset[] = Array.isArray(data.assets) ? data.assets : [];
        dmg = assets.find((asset) => DMG_PATTERN.test(asset.name));
        version = String(data.tag_name ?? '').replace(/^v/, '');
        if (typeof data.html_url === 'string' && data.html_url) notesUrl = data.html_url;
        state = dmg ? 'ready' : 'missing';
      } catch {
        if (!disposed) state = 'error';
      } finally {
        clearTimeout(timeout);
      }
    }
    void loadRelease();
    return () => { disposed = true; clearTimeout(timeout); controller.abort(); };
  });

  function formatSize(bytes: number): string {
    if (!Number.isFinite(bytes) || bytes <= 0) return '';
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
</script>

<div class="rv-release">
  <div class="rv-release-heading"><img src="/app-icon.png" width="62" height="62" alt="" /><div><span class="rv-release-label">{t('download.stable')}</span><strong>{t('download.platform')}</strong><small>{t('hero.requirements')}</small></div></div>
  <div class="rv-release-content" aria-live="polite">
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
  <div class="rv-release-trust"><Icon name="shield" size={15} />{t('download.verified')}</div>
</div>
