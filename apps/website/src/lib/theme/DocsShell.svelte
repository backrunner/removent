<script lang="ts">
  import { Article, TableOfContents } from 'svedocs/theme';
  import { resolveLocalizedHref } from 'svedocs/theme/headless';
  import type { SvedocsDocsShellProps } from 'svedocs/theme/types';
  import DocsNavigation from './DocsNavigation.svelte';
  import Icon from '../Icon.svelte';
  let { page, navigationTree = [], content, context, tocController, themeComponents = {} }: SvedocsDocsShellProps = $props();
  let overview = $derived(page.scopePath === '/docs');
  let tocOpen = $state(false);
  $effect(() => { page.routePath; tocOpen = false; });
  const shortcuts = [
    { key: 'install', icon: 'down', href: '/docs/installation' },
    { key: 'pair', icon: 'monitor', href: '/docs/connecting' },
    { key: 'relay', icon: 'network', href: '/docs/relay' }
  ];
</script>
<div class="rv-docs-shell">
  <aside class="rv-docs-sidebar" aria-label={context.t('nav.documentation')}>
    <div class="rv-docs-label"><span class="rv-section-dot" aria-hidden="true"></span>{context.t('docs.library')}</div>
    <nav><DocsNavigation items={navigationTree} currentPath={page.routePath} {context} /></nav>
    <a class="rv-sidebar-release" href="https://github.com/backrunner/removent/releases/tag/v0.1.3-beta.1">
      <span class="rv-release-label">{context.t('docs.release')}</span>
      <strong>0.1.3 <span>Beta 1</span><Icon name="arrow" size={16} /></strong>
      <span>{context.t('download.notes')}</span>
    </a>
  </aside>
  <main id="content" class="rv-docs-main">
    <Article {page} {content} {context} {themeComponents}>
      <svelte:fragment slot="doc-header" let:breadcrumbs>
        <header class="sd-doc-header rv-doc-heading">
          <nav class="sd-doc-eyebrow" aria-label={context.t('article.breadcrumb')}>
            {#each breadcrumbs as item, i}
              {#if i > 0}<span aria-hidden="true">/</span>{/if}<a href={item.path}>{item.label}</a>
            {/each}
            <span class="rv-heading-rule" aria-hidden="true"></span><span class="rv-doc-format">{context.t('docs.guide')}</span>
          </nav>
          <h1>{page.title}</h1>
          {#if page.description}<p class="sd-doc-lede">{page.description}</p>{/if}
        </header>
        {#if overview}
          <div class="rv-doc-shortcuts">
            {#each shortcuts as item, index}
              <a href={resolveLocalizedHref(item.href, context)}>
                <span class="rv-shortcut-icon"><Icon name={item.icon} size={21} /><small>0{index + 1}</small></span>
                <strong>{context.t(`docs.shortcut.${item.key}`)}</strong><Icon name="arrow" size={17} />
              </a>
            {/each}
          </div>
        {/if}
        {#if page.headings.length}
        <details class="rv-mobile-toc" bind:open={tocOpen}>
          <summary>{context.t('toc.label')}<Icon name="down" size={15} /></summary>
          <nav aria-label={context.t('toc.label')}>
            {#each page.headings as heading}
              <a class="sd-toc-link sd-depth-{heading.depth}" href={'#' + heading.id}
                onclick={() => { tocController?.activate(heading.id); tocOpen = false; }}>{heading.text}</a>
            {/each}
          </nav>
        </details>
        {/if}
      </svelte:fragment>
    </Article>
  </main>
  <div class="rv-docs-toc">
    <TableOfContents {page} controller={tocController} {context} />
    <div class="rv-doc-resources">
      <a href="https://github.com/backrunner/removent"><Icon name="layers" size={15} />{context.t('site.github')}<span aria-hidden="true">↗</span></a>
      <a href={`${page.routePath.replace(/\/$/, '')}/index.md`}><Icon name="book" size={15} />{context.t('docs.markdown')}<span aria-hidden="true">↗</span></a>
    </div>
  </div>
</div>
