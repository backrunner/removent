import componentLoaders from 'virtual:svedocs/component-loaders';
import layoutLoaders from 'virtual:svedocs/layout-loaders';
import { loadSvedocsPage } from 'svedocs/routes';
import { error, redirect } from '@sveltejs/kit';
import pageLoaders from 'virtual:svedocs/page-loaders';
import pages from 'virtual:svedocs/page-index';
import tree from 'virtual:svedocs/tree';
import config from 'virtual:svedocs/config';
import { svedocsPagePrerender } from 'svedocs/cloudflare';
import type { SvedocsPage } from 'svedocs/core';
import { createSvedocsRouteEntries, resolveSvedocsPageRoute } from 'svedocs/routes';
import type { PageLoad } from './$types';

export const prerender = svedocsPagePrerender(undefined, config);

export function entries() {
  return createSvedocsRouteEntries(pages, config)
    .map((path) => ({ path: path.replace(/^\//, '') }));
}

export const load: PageLoad = async ({ params }) => {
  const routePath = `/${params.path ?? ''}`.replace(/\/$/, '') || '/';
  const resolution = resolveSvedocsPageRoute(routePath, pages, config);
  if (resolution.status === 'redirect') redirect(307, resolution.location);
  if (resolution.status === 'missing') error(404, `No page found for ${routePath}`);
  const pageIndex = resolution.page;
  const loaded = await loadSvedocsPage(pageIndex, { pages: pageLoaders, components: componentLoaders, layouts: layoutLoaders });
  const { page } = loaded;
  return { ...loaded, pages: mergeCurrentPage(pages, page), search: [], tree, config };
};

function mergeCurrentPage(pages: SvedocsPage[], current: SvedocsPage): SvedocsPage[] {
  return pages.map((page) => page.id === current.id ? current : page);
}
