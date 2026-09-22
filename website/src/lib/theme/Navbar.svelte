<script lang="ts">
  import { SearchDialog, ScopeSwitcher, ThemeToggle, SidebarTree } from 'svedocs/theme';
  import { resolveLocalizedHref } from 'svedocs/theme/headless';
  import type { SvedocsNavbarProps } from 'svedocs/theme/types';
  import Icon from '../Icon.svelte';
  let { context, mobileTree = [], mobileCurrentPath = '', mobileMenuId = 'rv-menu', mobileMenuOpen = false, onToggleMobileMenu = () => {}, onCloseMobileMenu = () => {} }: SvedocsNavbarProps = $props();
</script>
<header class="rv-header">
  <div class="rv-nav-inner">
    <a class="rv-brand" href={resolveLocalizedHref('/', context)} aria-label={context.t('site.home')}><img src="/app-icon.png" width="34" height="34" alt="" /><span>Removent</span></a>
    <nav class="rv-desktop-nav" aria-label={context.t('nav.primary')}>
      {#each context.config.theme.nav as link}
        <a href={resolveLocalizedHref(link.href, context)} aria-current={context.page?.scopePath === link.href || (link.href === '/docs' && context.isDocsPage) ? 'page' : undefined}>{link.labelKey ? context.t(link.labelKey) : link.label}</a>
      {/each}
    </nav>
    <div class="rv-nav-tools">
      <div class="rv-search-control"><Icon name="search" size={18} /><SearchDialog records={context.search} loadRecords={context.loadSearch} scope={context.searchScope} provider={context.config.search.provider} buildMode={context.config.build.mode} {context} /></div>
      <ScopeSwitcher page={context.page} pages={context.pages} locales={context.config.i18n.locales} {context} />
      <ThemeToggle {context} />
      <a class="rv-source" href="https://github.com/backrunner/removent" aria-label={context.t('site.github')} title={context.t('site.github')}>
        <svg viewBox="0 0 24 24" width="19" height="19" fill="currentColor" aria-hidden="true"><path d="M12 2a10 10 0 0 0-3.16 19.49c.5.09.68-.22.68-.48v-1.86c-2.78.6-3.37-1.18-3.37-1.18-.45-1.16-1.11-1.47-1.11-1.47-.91-.62.07-.61.07-.61 1 .07 1.53 1.03 1.53 1.03.89 1.53 2.34 1.09 2.91.83.09-.65.35-1.09.64-1.34-2.22-.25-4.55-1.11-4.55-4.94 0-1.09.39-1.99 1.03-2.69-.1-.26-.45-1.28.1-2.66 0 0 .84-.27 2.75 1.03a9.56 9.56 0 0 1 5 0c1.91-1.3 2.75-1.03 2.75-1.03.55 1.38.2 2.4.1 2.66.64.7 1.03 1.6 1.03 2.69 0 3.84-2.34 4.69-4.57 4.94.36.31.68.92.68 1.85v2.75c0 .26.18.58.69.48A10 10 0 0 0 12 2Z" /></svg>
      </a>
      <button class="rv-menu-toggle" type="button" aria-label={context.t(mobileMenuOpen ? 'nav.mobile.close' : 'nav.mobile.open')} aria-expanded={mobileMenuOpen} aria-controls={mobileMenuId} onclick={onToggleMobileMenu}><Icon name={mobileMenuOpen ? 'close' : 'menu'} /></button>
    </div>
  </div>
  {#if mobileMenuOpen}
    <div class="rv-mobile-menu" id={mobileMenuId}>
      <nav aria-label={context.t('nav.primary')}>
        {#each context.config.theme.nav as link}<a href={resolveLocalizedHref(link.href, context)} onclick={onCloseMobileMenu}>{link.labelKey ? context.t(link.labelKey) : link.label}</a>{/each}
      </nav>
      {#if mobileTree.length}<nav class="rv-mobile-docs" aria-label={context.t('nav.documentation')}><SidebarTree items={mobileTree} currentPath={mobileCurrentPath} /></nav>{/if}
    </div>
  {/if}
</header>
