import { createRobotsResponse } from 'svedocs/og';
import config from 'virtual:svedocs/config';
import type { RequestHandler } from './$types';

export const prerender = config.seo.robots;

export const GET: RequestHandler = ({ request }) => createRobotsResponse(config, request);
