import { test } from 'node:test';
import assert from 'node:assert/strict';
import { submitSupport, type SupportEnv } from '../src/lib/server/support-intake.ts';
import { validateSupportInput } from '../src/lib/support.ts';
import worker from '../worker.ts';

const origin = 'https://removent.pwp.sh';
const input = { email: ' Person@example.com ', topic: 'problem', locale: 'zh', subject: ' 无法打开 record ', content: '重现步骤与错误文字。', appVersion: '1.0', osVersion: '26', deviceModel: 'iPhone', turnstileToken: 'test-challenge' };
const env: SupportEnv = {
  ONFIRE_API_KEY: 'private-product-key', ONFIRE_PRODUCT_ID: 'removent-product',
  ONFIRE_TICKET_TYPES: JSON.stringify({ problem: 'problem-type', feature: 'feature-type', question: 'question-type', privacy: 'privacy-type' }),
  SUPPORT_RATE_LIMIT: { async limit() { return { success: true }; } }
};
const request = (body: unknown = input, headers: Record<string, string> = {}) => new Request(`${origin}/api/support`, { method: 'POST', headers: { origin, 'content-type': 'application/json', ...headers }, body: JSON.stringify(body) });
const ok = (data: object) => Response.json({ ok: true, data });

function upstream(overrides: Partial<Record<'identity' | 'form' | 'ticket', () => Response | Promise<Response>>> = {}) {
  const calls: { url: string; init: RequestInit }[] = [];
  const fetcher: typeof fetch = async (url, init = {}) => {
    calls.push({ url: String(url), init });
    assert.equal(init.redirect, 'manual');
    assert.ok(init.signal);
    if (String(url).endsWith('/tokens')) return overrides.identity?.() ?? ok({ productId: env.ONFIRE_PRODUCT_ID, token: 'private-customer-token' });
    if (String(url).includes('/ticket-types/')) return overrides.form?.() ?? ok({ ticketTypeId: 'problem-type', templateVersionId: 'latest-version' });
    if (String(url).endsWith('/tickets?lang=en')) return overrides.ticket?.() ?? ok({ ticketId: 'created-ticket' });
    throw new Error('Unexpected upstream URL');
  };
  return { calls, fetcher };
}

test('creates a ticket with server-owned routing, latest template and original language; exposes only the reference', async () => {
  const { calls, fetcher } = upstream();
  const response = await submitSupport(request({ ...input, productId: 'attacker', ticketTypeId: 'attacker', templateVersionId: 'attacker', priority: 'high' }), env, '192.0.2.1', fetcher);
  assert.equal(response.status, 201);
  assert.equal(response.headers.get('cache-control'), 'no-store');
  assert.deepEqual(await response.json(), { ticketId: 'created-ticket' });
  assert.equal(calls.length, 3);
  assert.deepEqual(JSON.parse(calls[0].init.body as string), { apiKey: env.ONFIRE_API_KEY, email: 'person@example.com' });
  assert.match(calls[1].url, /ticket-types\/problem-type\/form\?lang=en$/);
  assert.deepEqual(JSON.parse(calls[2].init.body as string), {
    ticketTypeId: 'problem-type', templateVersionId: 'latest-version', subject: '无法打开 record', content: input.content,
    metadata: { preferred_language: 'zh', app_version: '1.0', os_version: '26', device_model: 'iPhone' },
    turnstileToken: input.turnstileToken
  });
  assert.equal(new Headers(calls[2].init.headers).get('authorization'), 'Bearer private-customer-token');
  assert.equal(calls.filter(call => call.url.includes('siteverify')).length, 0);
});

test('rejects invalid, oversized, cross-origin and non-JSON inputs before contacting OnFire', async () => {
  const { calls, fetcher } = upstream();
  for (const [req, status] of [
    [request(input, { origin: 'https://other.example' }), 403],
    [request(input, { origin: '' }), 403],
    [request(input, { 'content-type': 'text/plain' }), 415],
    [request({ ...input, email: 'invalid' }), 400],
    [request({ ...input, locale: 'other' }), 400],
    [request({ ...input, topic: '../admin' }), 400],
    [request({ ...input, turnstileToken: '' }), 400],
    [request({ ...input, content: ' '.repeat(65000) }), 400],
    [new Request(`${origin}/api/support`), 405]
  ] as const) assert.equal((await submitSupport(req, env, 'ip', fetcher)).status, status);
  assert.equal(calls.length, 0);
  assert.equal(validateSupportInput({ ...input, subject: '   ' }), null);
  assert.equal(validateSupportInput({ ...input, appVersion: [] }), null);
});

test('fails closed for missing configuration, broken limits and wrong product mappings', async () => {
  const { calls, fetcher } = upstream();
  assert.equal((await submitSupport(request(), undefined, 'ip', fetcher)).status, 503);
  assert.equal((await submitSupport(request(), { ...env, ONFIRE_TICKET_TYPES: '{}' }, 'ip', fetcher)).status, 503);
  assert.equal((await submitSupport(request(), { ...env, SUPPORT_RATE_LIMIT: { async limit() { throw new Error('unavailable'); } } }, 'ip', fetcher)).status, 503);
  assert.equal((await submitSupport(request(), { ...env, SUPPORT_RATE_LIMIT: { async limit({ key }) { assert.equal(key, 'removent:support:ip'); return { success: false }; } } }, 'ip', fetcher)).status, 429);
  assert.equal(calls.length, 0);
  const mismatch = upstream({ identity: () => ok({ productId: 'wrong-product', token: 'secret' }) });
  assert.equal((await submitSupport(request(), env, 'ip', mismatch.fetcher)).status, 502);
  assert.equal(mismatch.calls.length, 1);
});

test('distinguishes CAPTCHA rejection, form changes and upstream throttling', async () => {
  for (const [status, upstreamError, error] of [[400, 'CAPTCHA verification failed', 'verification'], [400, 'Template fields are invalid', 'form'], [409, 'Template version has been invalidated', 'form'], [429, 'Slow down', 'rate_limit']] as const) {
    const mock = upstream({ ticket: () => Response.json({ ok: false, error: upstreamError }, { status }) });
    const response = await submitSupport(request(), env, 'ip', mock.fetcher);
    assert.equal(response.status, status);
    assert.deepEqual(await response.json(), { error });
    assert.equal(mock.calls.length, 3);
  }
});

test('never retries an uncertain ticket write or exposes upstream errors/secrets', async () => {
  for (const failure of [() => { throw new Error('private-key network failure'); }, () => new Response('private-token', { status: 502 }), () => ok({ unexpected: true })]) {
    const mock = upstream({ ticket: failure });
    const response = await submitSupport(request(), env, 'ip', mock.fetcher);
    assert.deepEqual(await response.json(), { error: 'uncertain' });
    assert.equal(mock.calls.length, 3);
  }
  const early = upstream({ form: () => { throw new Error('timeout'); } });
  assert.deepEqual(await (await submitSupport(request(), env, 'ip', early.fetcher)).json(), { error: 'unavailable' });
  assert.equal(early.calls.length, 2);
});

test('worker mounts only the support intake route and keeps assets and 404 responses intact', async () => {
  const bindings = { ...env, ASSETS: { async fetch(req: Request) { return new Response(new URL(req.url).pathname, { status: 404 }); } } };
  for (const path of ['/api/support', '/api/support/']) assert.equal((await worker.fetch(new Request(origin + path), bindings)).status, 405);
  for (const path of ['/docs/', '/zh/support/', '/api/support/tickets']) {
    const response = await worker.fetch(new Request(origin + path), bindings);
    assert.equal(response.status, 404);
    assert.equal(await response.text(), path);
  }
});

// Workers' native fetch requires its global receiver, unlike Node's permissive fetch.
test('default upstream fetch retains the global receiver in Workers', async () => {
  const original = globalThis.fetch;
  const mock = upstream();
  globalThis.fetch = function (this: unknown, url, init) {
    assert.equal(this, globalThis);
    return mock.fetcher(url, init);
  };
  try {
    const response = await submitSupport(request(), env, 'ip');
    assert.equal(response.status, 201);
    assert.equal(mock.calls.length, 3);
  } finally { globalThis.fetch = original; }
});

test('rejects identity redirects without forwarding credentials or creating a ticket', async () => {
  const mock = upstream({ identity: () => new Response(null, { status: 307, headers: { location: 'https://other.example/collect' } }) });
  const response = await submitSupport(request(), env, 'ip', mock.fetcher);
  assert.equal(response.status, 502);
  assert.deepEqual(await response.json(), { error: 'unavailable' });
  assert.equal(mock.calls.length, 1);
});
