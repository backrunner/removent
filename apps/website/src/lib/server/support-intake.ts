import { validateSupportInput } from '../support.ts';
const siteUrl = 'https://removent.pwp.sh';

export interface SupportEnv {
  ONFIRE_API_KEY: string;
  ONFIRE_PRODUCT_ID: string;
  /** Server-owned mapping: problem/feature/question/privacy -> OnFire ticket type ID. */
  ONFIRE_TICKET_TYPES: string;
  SUPPORT_RATE_LIMIT: { limit(input: { key: string }): Promise<{ success: boolean }> };
}
const onfire = 'https://support.alkinum.io/api/toc';
const headers = { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' };
const reply = (status: number, body: object) => new Response(JSON.stringify(body), { status, headers });

class UpstreamError extends Error {
  status: number;
  stage: string;
  reason: string;
  constructor(status: number, stage: string, reason = '') { super('Support service request failed'); this.status = status; this.stage = stage; this.reason = reason; }
}

async function boundedJson(request: Request | Response, limit: number): Promise<unknown> {
  if (!request.body) throw new Error('Missing body');
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > limit) throw new Error('Body too large');
      chunks.push(value);
    }
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  } finally { reader.releaseLock(); }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  return JSON.parse(new TextDecoder().decode(bytes));
}

/** Submission only: customer JWTs and account history never leave this server. */
export async function submitSupport(request: Request, env: SupportEnv | undefined, clientIp: string, upstreamFetch: typeof fetch = fetch.bind(globalThis)): Promise<Response> {
  if (request.method !== 'POST') return reply(405, { error: 'method' });
  // Browser CSRF boundary, not proof of caller identity: non-browser clients can forge Origin.
  if (new URL(request.url).origin !== siteUrl || request.headers.get('origin') !== siteUrl) return reply(403, { error: 'origin' });
  if (request.headers.get('content-type')?.split(';')[0].trim() !== 'application/json') return reply(415, { error: 'media' });
  if (!env?.ONFIRE_API_KEY || !env.ONFIRE_PRODUCT_ID || !env.ONFIRE_TICKET_TYPES || !env.SUPPORT_RATE_LIMIT) {
    return reply(503, { error: 'unavailable' });
  }
  let mapping: Record<string, string>;
  try {
    mapping = JSON.parse(env.ONFIRE_TICKET_TYPES);
    if (!mapping || typeof mapping !== 'object') throw new Error('Invalid config');
    const allowed = await env.SUPPORT_RATE_LIMIT.limit({ key: `removent:support:${clientIp}` });
    if (!allowed.success) return reply(429, { error: 'rate_limit' });
  } catch { return reply(503, { error: 'unavailable' }); }
  let input;
  try { input = validateSupportInput(await boundedJson(request, 64000)); }
  catch { return reply(400, { error: 'invalid' }); }
  if (!input) return reply(400, { error: 'invalid' });
  const typeId = mapping[input.topic];
  if (typeof typeId !== 'string' || !/^[a-zA-Z0-9_-]{1,100}$/.test(typeId)) return reply(503, { error: 'unavailable' });

  let lastStage = 'identity';
  async function call(path: string, stage: string, body?: object, token?: string): Promise<Record<string, unknown>> {
    lastStage = stage;
    const response = await upstreamFetch(`${onfire}${path}`, {
      method: body ? 'POST' : 'GET',
      headers: { 'Content-Type': 'application/json', 'Accept-Language': 'en', 'User-Agent': 'RemoventSupport/0.1.3', ...(token ? { Authorization: `Bearer ${token}` } : {}) },
      body: body ? JSON.stringify(body) : undefined,
      // Workers supports only follow/manual. Reject all redirects below.
      redirect: 'manual',
      signal: AbortSignal.timeout(stage === 'ticket' ? 60000 : 15000)
    });
    if (!response.ok) {
      let reason = '';
      try { const body = await boundedJson(response, 16000) as { error?: string }; reason = typeof body?.error === 'string' ? body.error : ''; } catch { /* Do not forward upstream bodies. */ }
      throw new UpstreamError(response.status, stage, reason);
    }
    if (!response.headers.get('content-type')?.includes('application/json')) throw new UpstreamError(response.status, stage, 'non_json');
    const data = await boundedJson(response, 128000) as { ok?: boolean; data?: Record<string, unknown> };
    if (!data.ok || !data.data || typeof data.data !== 'object') throw new UpstreamError(502, stage);
    return data.data;
  }
  let ticketAttempted = false;
  try {
    const identity = await call('/tokens', 'identity', { apiKey: env.ONFIRE_API_KEY, email: input.email });
    if (identity.productId !== env.ONFIRE_PRODUCT_ID || typeof identity.token !== 'string' || !identity.token) throw new UpstreamError(502, 'identity');
    const form = await call(`/ticket-types/${encodeURIComponent(typeId)}/form?lang=en`, 'form', undefined, identity.token);
    if (form.ticketTypeId !== typeId || typeof form.templateVersionId !== 'string') throw new UpstreamError(502, 'form');
    const metadata: Record<string, string> = { preferred_language: input.locale };
    if (input.appVersion) metadata.app_version = input.appVersion;
    if (input.osVersion) metadata.os_version = input.osVersion;
    if (input.deviceModel) metadata.device_model = input.deviceModel;
    // OnFire verifies the single-use Turnstile token. Do not consume it here.
    ticketAttempted = true;
    const result = await call('/tickets?lang=en', 'ticket', {
      ticketTypeId: typeId, templateVersionId: form.templateVersionId,
      subject: input.subject, content: input.content, metadata, turnstileToken: input.turnstileToken
    }, identity.token);
    if (typeof result.ticketId !== 'string' || !result.ticketId) throw new UpstreamError(502, 'ticket');
    return reply(201, { ticketId: result.ticketId });
  } catch (error) {
    // Diagnostic categories only: never log credentials, messages, emails or response bodies.
    console.warn('Support upstream failure', error instanceof UpstreamError ? { stage: error.stage, status: error.status, category: error.reason === 'non_json' ? 'non_json' : 'http' } : { stage: lastStage, category: error instanceof Error && ['TypeError', 'SyntaxError', 'TimeoutError', 'AbortError'].includes(error.name) ? error.name : 'connection' });
    if (error instanceof UpstreamError && error.status === 429) return reply(429, { error: 'rate_limit' });
    if (error instanceof UpstreamError && error.stage === 'ticket' && error.status === 400) {
      return reply(400, { error: error.reason === 'CAPTCHA verification failed' ? 'verification' : 'form' });
    }
    if (error instanceof UpstreamError && error.stage === 'ticket' && error.status === 409) return reply(409, { error: 'form' });
    // Never retry a ticket POST automatically: a timeout may occur after creation.
    return reply(502, { error: ticketAttempted ? 'uncertain' : 'unavailable' });
  }
}
