<script lang="ts">
  import type { SvedocsFooterProps } from 'svedocs/theme/types';
  import { resolveLocalizedHref } from 'svedocs/theme/headless';
  let { context }: SvedocsFooterProps = $props();
</script>
<footer class="rv-footer">
  <div><a class="rv-brand" href={resolveLocalizedHref('/', context)}><img src="/app-icon.png" width="28" height="28" alt="" /><span>Removent</span></a><p>{context.t('site.footer')}</p></div>
  <nav aria-label={context.t('nav.footer')}>
    <a href={resolveLocalizedHref('/docs', context)}>{context.t('nav.documentation')}</a>
    <a href="https://github.com/backrunner/removent/releases">{context.t('site.releases')}</a>
    <a href={resolveLocalizedHref('/privacy', context)}>{context.t('site.privacy')}</a>
    <a href="https://github.com/backrunner/removent/blob/main/LICENSE">{context.t('site.license')}</a>
  </nav>
  <div class="rv-footer-bottom"><span>{context.t('site.built')}</span><span>© {new Date().getFullYear()} <a href="https://github.com/backrunner">Alkinum</a></span></div>
</footer>
