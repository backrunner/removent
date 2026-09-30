import componentLoaders from 'virtual:svedocs/component-loaders';
import layoutLoaders from 'virtual:svedocs/layout-loaders';
import { loadSvedocsPage, resolveSvedocsPageRoute } from 'svedocs/routes';
import pageLoaders from 'virtual:svedocs/page-loaders';
import pages from 'virtual:svedocs/page-index';
import tree from 'virtual:svedocs/tree';
import config from 'virtual:svedocs/config';
import { svedocsPagePrerender } from 'svedocs/cloudflare';
import { error, redirect } from '@sveltejs/kit';
import type { SvedocsPage } from 'svedocs/core';
import type { PageLoad } from './$types';

export const prerender = svedocsPagePrerender(undefined, config);

export const load: PageLoad = async () => {
  const resolution = resolveSvedocsPageRoute('/', pages, config);
  if (resolution.status === 'redirect') redirect(307, resolution.location);
  if (resolution.status === 'missing') error(404, 'No homepage found in the svedocs content manifest.');
  const pageIndex = resolution.page;
  const loaded = await loadSvedocsPage(pageIndex, { pages: pageLoaders, components: componentLoaders, layouts: layoutLoaders });
  return { ...loaded, pages: mergeCurrentPage(pages, loaded.page), search: [], tree, config };
};

function mergeCurrentPage(pages: SvedocsPage[], current: SvedocsPage): SvedocsPage[] {
  return pages.map((page) => page.id === current.id ? current : page);
}
