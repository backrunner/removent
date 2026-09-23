<script lang="ts">
  import { Article, SidebarTree, TableOfContents } from 'svedocs/theme';
  import type { SvedocsDocsShellProps } from 'svedocs/theme/types';
  let { page, navigationTree = [], content, context, tocController, themeComponents = {} }: SvedocsDocsShellProps = $props();
</script>
<div class="rv-docs-shell">
  <aside class="rv-docs-sidebar" aria-label={context.t('nav.documentation')}>
    <nav><SidebarTree items={navigationTree} currentPath={page.routePath} /></nav>
    <a class="rv-sidebar-source" href="https://github.com/backrunner/removent">GitHub <span aria-hidden="true">↗</span></a>
  </aside>
  <main id="content" class="rv-docs-main"><Article {page} {content} {context} {themeComponents} /></main>
  <div class="rv-docs-toc"><TableOfContents {page} controller={tocController} {context} /></div>
</div>
