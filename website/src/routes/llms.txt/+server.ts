import { createLlmsTxtResponse } from 'svedocs/agent';
import config from 'virtual:svedocs/config';
import markdown from 'virtual:svedocs/markdown';
import pages from 'virtual:svedocs/page-index';
import type { RequestHandler } from './$types';

export const prerender = config.agent.enabled && config.agent.llms && config.agent.markdown;

export const GET: RequestHandler = ({ request }) => createLlmsTxtResponse(config, pages, markdown, request);
