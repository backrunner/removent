import { createPageMarkdownEntries, createPageMarkdownResponse } from 'svedocs/agent';
import config from 'virtual:svedocs/config';
import markdown from 'virtual:svedocs/markdown';
import pages from 'virtual:svedocs/page-index';
import type { EntryGenerator, RequestHandler } from './$types';

export const prerender = config.agent.enabled && config.agent.markdown;

export const entries: EntryGenerator = () => createPageMarkdownEntries(config, pages, markdown);

export const GET: RequestHandler = ({ params, request }) =>
  createPageMarkdownResponse(config, pages, markdown, `/${params.path}/index.md`, request);
