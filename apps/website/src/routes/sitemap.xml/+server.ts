import { createSitemapResponse } from 'svedocs/og';
import config from 'virtual:svedocs/config';
import pages from 'virtual:svedocs/page-index';
import type { RequestHandler } from './$types';

export const prerender = config.seo.sitemap;

export const GET: RequestHandler = ({ request }) => createSitemapResponse(config, pages, request);
