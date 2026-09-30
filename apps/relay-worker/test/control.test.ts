import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash, generateKeyPairSync, sign, randomBytes } from "node:crypto";
import { authenticate, ingress, claimAdmission, protocol, RelayControl, validateConfig, type RelayConfig } from "../src/control.js";

const tokens = { host: "11".repeat(32), client: "22".repeat(32), admin: "33".repeat(32) };
const hash = (token: string) => createHash("sha256").update(Buffer.from(token, "hex")).digest("hex");
const env: RelayConfig = {
  RELAY_ROOM: "office", RELAY_HOST_TOKEN_SHA256: hash(tokens.host), RELAY_CLIENT_TOKEN_SHA256: hash(tokens.client), RELAY_ADMIN_TOKEN_SHA256: hash(tokens.admin),
};
const admin = (action: string, token = tokens.admin, method = action === "status" ? "GET" : "POST") => new Request(`https://relay.example/admin/${action}`, { method, headers: { authorization: `Bearer ${token}` } });
const identities = { host: generateKeyPairSync("ed25519"), client: generateKeyPairSync("ed25519") };
const keyHex = (role: "host" | "client") => Buffer.from(identities[role].publicKey.export({ format: "der", type: "spki" }).subarray(-32)).toString("hex");
const tunnel = (role: "host" | "client", token = tokens[role]) => {
  const time = String(Math.floor(Date.now() / 1000)), nonce = Buffer.from(randomBytes(32)).toString("hex");
  const audience = "removent://relay.example:443";
  const message = `removent-relay-admission-v1\n${audience}\noffice\n${role}\n${time}\n${nonce}`;
  const headers: Record<string, string> = {
    upgrade: "websocket", "sec-websocket-protocol": protocol, "x-removent-room": "office", "x-removent-role": role,
    "x-removent-key": keyHex(role), "x-removent-time": time, "x-removent-nonce": nonce, "x-removent-audience": audience,
    "x-removent-signature": Buffer.from(sign(null, Buffer.from(message), identities[role].privateKey)).toString("hex"),
  };
  if (token) headers.authorization = `Bearer ${token}`;
  return new Request("https://relay.example/v1/tunnel", { headers });
};

test("blank/invalid idle values cannot silently disable automatic shutdown", () => {
  assert.equal(validateConfig(env), 300);
  assert.equal(validateConfig({ ...env, RELAY_IDLE_SECONDS: "0" }), 0);
  for (const value of ["", " ", "1", "59", "86401", "-1", "300.5", "NaN"]) {
    assert.throws(() => validateConfig({ ...env, RELAY_IDLE_SECONDS: value }));
  }
});

test("management credentials, role tokens, room, method and URL are isolated", async () => {
  for (const action of ["start", "stop", "status"]) {
    assert.equal(await authenticate(admin(action), env), action);
    for (const token of [tokens.host, tokens.client, "00".repeat(32)]) {
      assert.equal((await authenticate(admin(action, token), env) as Response).status, 401);
    }
  }
  assert.equal((await authenticate(admin("stop", tokens.admin, "GET"), env) as Response).status, 405);
  assert.equal(await authenticate(tunnel("host"), env), "host");
  assert.equal(await authenticate(tunnel("client"), env), "client");
  for (const token of [tokens.client, tokens.admin]) assert.equal((await authenticate(tunnel("host", token), env) as Response).status, 401);
  const oldProtocol = tunnel("client"); oldProtocol.headers.set("sec-websocket-protocol", "removent-relay.ws.v2");
  assert.equal((await authenticate(oldProtocol, env) as Response).status, 400);
  const oldProof = tunnel("client");
  const oldMessage = `removent-relay-admission-v2\nremovent://relay.example:443\noffice\nclient\n${oldProof.headers.get("x-removent-time")}\n${oldProof.headers.get("x-removent-nonce")}`;
  oldProof.headers.set("x-removent-signature", Buffer.from(sign(null, Buffer.from(oldMessage), identities.client.privateKey)).toString("hex"));
  assert.equal((await authenticate(oldProof, env) as Response).status, 401);
  const otherRoom = tunnel("client"); otherRoom.headers.set("x-removent-room", "another");
  assert.equal((await authenticate(otherRoom, env) as Response).status, 401);
  const origin = admin("stop"); origin.headers.set("origin", "https://other.example");
  assert.equal((await authenticate(origin, env) as Response).status, 400);
  assert.equal((await authenticate(new Request("https://relay.example/admin/status?token=secret"), env) as Response).status, 400);
});

function fixture() {
  let running = false, starts = 0, destroys = 0, forwards = 0;
  const stored = new Map<string, boolean>();
  const storage = { get: async (key: string) => stored.get(key), put: async (key: string, value: boolean) => { stored.set(key, value); } };
  const runtime = {
    running: () => running,
    start: async (_signal: AbortSignal) => { starts++; running = true; },
    destroy: async () => { destroys++; running = false; },
    forward: async (_request: Request) => { assert.equal(running, true); forwards++; return new Response("forwarded"); },
  };
  return { storage, runtime, counts: () => ({ starts, destroys, forwards }), sleep: () => { running = false; } };
}

test("starts disabled; status and daemon retries never boot; stop survives a DO restart", async () => {
  const f = fixture();
  let control = new RelayControl(f.storage, f.runtime); await control.restore();
  for (let i = 0; i < 5; i++) {
    assert.equal((await control.handle("host", tunnel("host"))).status, 503);
    assert.equal((await control.handle("client", tunnel("client"))).status, 503);
    assert.equal((await control.handle("status", admin("status"))).status, 200);
  }
  assert.equal(f.counts().starts, 0);
  assert.equal((await control.handle("start", admin("start"))).status, 200);
  assert.equal((await control.handle("host", tunnel("host"))).status, 200);
  await control.handle("stop", admin("stop"));
  control = new RelayControl(f.storage, f.runtime); await control.restore();
  assert.equal((await control.handle("host", tunnel("host"))).status, 503);
  assert.equal((await control.handle("client", tunnel("client"))).status, 503);
  assert.equal(f.counts().starts, 1);
  await control.handle("start", admin("start"));
  assert.equal(f.counts().starts, 2);
});

test("idle container wakes for a controller, never for a host or status poll", async () => {
  const f = fixture(); const control = new RelayControl(f.storage, f.runtime); await control.restore();
  await control.handle("start", admin("start")); f.sleep();
  assert.equal((await control.handle("host", tunnel("host"))).status, 503);
  assert.equal((await control.handle("status", admin("status"))).status, 200);
  assert.equal(f.counts().starts, 1);
  assert.equal((await control.handle("client", tunnel("client"))).status, 200);
  assert.equal(f.counts().starts, 2);
});

test("stop aborts an in-flight cold start and remains durable", async () => {
  const f = fixture();
  let entered!: () => void;
  const started = new Promise<void>(resolve => { entered = resolve; });
  f.runtime.start = async signal => {
    entered();
    await new Promise<void>((_, reject) => signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true }));
  };
  const control = new RelayControl(f.storage, f.runtime); await control.restore();
  const pending = control.handle("start", admin("start"));
  const failed = assert.rejects(pending, /aborted/);
  await started;
  await control.handle("stop", admin("stop")); await failed;
  assert.equal(await f.storage.get("enabled"), false);
  assert.equal((await control.handle("client", tunnel("client"))).status, 503);
});

test("a queued start cannot undo a newer stop; forwarding a dying socket cannot auto-boot", async () => {
  const f = fixture(); const control = new RelayControl(f.storage, f.runtime); await control.restore();
  await Promise.all([control.handle("start", admin("start")), control.handle("stop", admin("stop"))]);
  assert.equal(await f.storage.get("enabled"), false);
  assert.equal(f.counts().starts, 0);
  await control.handle("start", admin("start"));
  f.runtime.forward = async () => { f.sleep(); return new Response("socket closed", { status: 503 }); };
  assert.equal((await control.handle("host", tunnel("host"))).status, 503);
  assert.equal((await control.handle("host", tunnel("host"))).status, 503);
  assert.equal(f.counts().starts, 1);
});

test("a later explicit start wins over an earlier pending stop", async () => {
  const f = fixture(); const control = new RelayControl(f.storage, f.runtime); await control.restore();
  await control.handle("start", admin("start"));
  await Promise.all([control.handle("stop", admin("stop")), control.handle("start", admin("start"))]);
  assert.equal(f.runtime.running(), true);
  assert.equal(await f.storage.get("enabled"), true);
  assert.equal(f.counts().starts, 2);
});

test("failed container destruction still persists disabled admission", async () => {
  const f = fixture(); const control = new RelayControl(f.storage, f.runtime); await control.restore();
  await control.handle("start", admin("start"));
  f.runtime.destroy = async () => { throw new Error("platform unavailable"); };
  await assert.rejects(control.handle("stop", admin("stop")));
  const restored = new RelayControl(f.storage, f.runtime); await restored.restore();
  assert.equal((await restored.handle("host", tunnel("host"))).status, 503);
  assert.equal((await restored.handle("client", tunnel("client"))).status, 503);
});


test("credential-free device admission binds role, audience, time and replay nonce", async () => {
  const keyed = { ...env, RELAY_HOST_TOKEN_SHA256: "", RELAY_CLIENT_TOKEN_SHA256: "", RELAY_HOST_PUBLIC_KEYS: keyHex("host"), RELAY_CLIENT_PUBLIC_KEYS: keyHex("client") };
  assert.equal(await authenticate(tunnel("host", ""), keyed), "host");
  const valid = tunnel("client", "");
  assert.equal(await authenticate(valid, keyed), "client");
  for (const [header, value] of [["x-removent-role", "host"], ["x-removent-key", keyHex("host")], ["x-removent-audience", "removent://evil.example:443"], ["x-removent-time", "1"], ["x-removent-signature", "00".repeat(64)]]) {
    const bad = new Request(valid); bad.headers.set(header!, value!);
    assert.equal((await authenticate(bad, keyed) as Response).status, 401);
  }
  const ledger = claimAdmission(valid, {})!;
  assert.ok(ledger);
  const full = Object.fromEntries(Array.from({ length: 512 }, (_, i) => [`cached-${i}`, Date.now() / 1000 + 121]));
  assert.equal(claimAdmission(valid, full), undefined, "ledger admission must remain bounded");
  assert.equal(claimAdmission(valid, ledger), undefined);
  assert.equal(claimAdmission(valid, JSON.parse(JSON.stringify(ledger))), undefined, "replay denied after restore");
  assert.throws(() => validateConfig({ ...keyed, RELAY_CLIENT_PUBLIC_KEYS: "" }));
  assert.throws(() => validateConfig({ ...keyed, RELAY_CLIENT_PUBLIC_KEYS: keyHex("host") }));
});


test("denied source never reaches the DO even with valid device or admin credentials", async () => {
  const restricted = { ...env, RELAY_ALLOWED_CIDRS: "203.0.113.0/24" };
  let forwarded = 0;
  const forward = async () => { forwarded++; return new Response("accepted"); };
  for (const req of [tunnel("host"), tunnel("client"), admin("start")]) {
    Object.defineProperty(req, "cf", { value: { colo: "TEST" } });
    req.headers.set("cf-connecting-ip", "198.51.100.1");
    req.headers.set("x-forwarded-for", "203.0.113.4");
    assert.equal((await ingress(req, restricted, forward)).status, 403);
    assert.equal(forwarded, 0);
  }
  const allowed = tunnel("client");
  Object.defineProperty(allowed, "cf", { value: { colo: "TEST" } });
  allowed.headers.set("cf-connecting-ip", "203.0.113.4");
  assert.equal((await ingress(allowed, restricted, forward)).status, 200);
  assert.equal(forwarded, 1);
});
