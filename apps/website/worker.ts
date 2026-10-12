import { submitSupport, type SupportEnv } from './src/lib/server/support-intake.ts';

interface Env extends SupportEnv {
  ASSETS: { fetch(request: Request): Promise<Response> };
}

// Pages remain prerendered by svedocs. Only support intake needs server code.
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (path === '/api/support' || path === '/api/support/') {
      return submitSupport(request, env, request.headers.get('CF-Connecting-IP') ?? 'unknown');
    }
    return env.ASSETS.fetch(request);
  }
};
