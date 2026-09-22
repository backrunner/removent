import { allowedSource, networks } from "./network.js";

export interface RelayConfig {
  RELAY_ROOM: string;
  RELAY_ALLOWED_CIDRS?: string;
  RELAY_ADMIN_ALLOWED_CIDRS?: string;
  RELAY_HOST_TOKEN_SHA256: string;
  RELAY_CLIENT_TOKEN_SHA256: string;
  RELAY_ADMIN_TOKEN_SHA256: string;
  RELAY_IDLE_SECONDS?: string;
  RELAY_MAX_CONNECTIONS?: string;
  RELAY_MAX_CLIENTS_PER_ROOM?: string;
  RELAY_MAX_BYTES_PER_SECOND?: string;
  RELAY_HOST_PUBLIC_KEYS?: string;
  RELAY_CLIENT_PUBLIC_KEYS?: string;
}

export type Role = "host" | "client";
export type Action = Role | "start" | "stop" | "status";
const hex = /^[0-9a-f]{64}$/i;
export const protocol = "removent-relay.ws.v1";

export function deviceKeys(value?: string): string[] {
  if (!value?.trim()) return [];
  const keys = value.split(",").map(k => k.trim().toLowerCase());
  if (keys.length > 256 || keys.some(k => !hex.test(k))) throw new Error("Invalid device keys");
  return keys;
}

export function resourceLimits(env: RelayConfig) {
  const number = (value: string | undefined, fallback: number, min: number, max: number) => {
    if (value === undefined) return fallback;
    const parsed = Number(value);
    if (!/^[0-9]+$/.test(value) || !Number.isSafeInteger(parsed) || parsed < min || parsed > max) throw new Error("Invalid resource limit");
    return parsed;
  };
  return {
    max_connections: number(env.RELAY_MAX_CONNECTIONS, 128, 1, 4096),
    max_clients_per_room: number(env.RELAY_MAX_CLIENTS_PER_ROOM, 4, 1, 64),
    max_bytes_per_second: number(env.RELAY_MAX_BYTES_PER_SECOND, 50_000_000, 2048, 10_000_000_000),
  };
}

export function validateConfig(env: RelayConfig): number {
  resourceLimits(env);
  networks(env.RELAY_ALLOWED_CIDRS);
  networks(env.RELAY_ADMIN_ALLOWED_CIDRS);
  const hostKeys = deviceKeys(env.RELAY_HOST_PUBLIC_KEYS), clientKeys = deviceKeys(env.RELAY_CLIENT_PUBLIC_KEYS);
  const hashes = [env.RELAY_HOST_TOKEN_SHA256, env.RELAY_CLIENT_TOKEN_SHA256, env.RELAY_ADMIN_TOKEN_SHA256].filter(Boolean);
  if (!/^[A-Za-z0-9_-]{1,64}$/.test(env.RELAY_ROOM) || !hex.test(env.RELAY_ADMIN_TOKEN_SHA256 ?? "") ||
      (!env.RELAY_HOST_TOKEN_SHA256 && !hostKeys.length) || (!env.RELAY_CLIENT_TOKEN_SHA256 && !clientKeys.length) ||
      hashes.some(h => !hex.test(h)) || new Set(hashes.map(h => h.toLowerCase())).size !== hashes.length || hostKeys.some(k => clientKeys.includes(k))) {
    throw new Error("Configure distinct role credentials or device keys, and an admin credential");
  }
  const rawIdle = env.RELAY_IDLE_SECONDS ?? "300";
  if (!/^(0|[1-9][0-9]*)$/.test(rawIdle)) throw new Error("Invalid idle timeout");
  const idle = Number(rawIdle);
  if (!Number.isInteger(idle) || (idle !== 0 && (idle < 60 || idle > 86400))) throw new Error("Invalid idle timeout");
  return idle;
}

async function authorized(request: Request, expected: string): Promise<boolean> {
  const token = request.headers.get("authorization")?.match(/^Bearer ([a-f0-9]{64})$/i)?.[1];
  if (!token) return false;
  const bytes = Uint8Array.from(token.match(/../g)!, part => parseInt(part, 16));
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  const target = Uint8Array.from(expected.match(/../g)!, part => parseInt(part, 16));
  let difference = 0;
  for (let i = 0; i < 32; i++) difference |= digest[i]! ^ target[i]!;
  return difference === 0;
}

export function reply(message: string, status: number): Response {
  return Response.json({ error: message }, { status, headers: { "cache-control": "no-store" } });
}

/** Public entrypoint: a denied network must not even allocate a DO stub. */
export async function ingress(request: Request, env: RelayConfig, forward: () => Promise<Response>): Promise<Response> {
  try {
    if (!allowedSource(request, env)) return reply("Source network is not allowed", 403);
    const action = await authenticate(request, env);
    if (action instanceof Response) return action;
    return await forward();
  } catch { return reply("Relay is temporarily unavailable", 503); }
}

/** Run before touching the DO/container, and again inside the DO fetch handler. */
export async function authenticate(request: Request, env: RelayConfig): Promise<Action | Response> {
  validateConfig(env);
  const url = new URL(request.url);
  if (url.search || request.headers.has("origin")) return reply("Invalid request", 400);
  if (url.protocol !== "https:" && !["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)) return reply("HTTPS required", 400);
  const admin = /^\/admin\/(start|stop|status)$/.exec(url.pathname)?.[1] as "start" | "stop" | "status" | undefined;
  if (admin) {
    if (request.method !== (admin === "status" ? "GET" : "POST")) return reply("Method not allowed", 405);
    return await authorized(request, env.RELAY_ADMIN_TOKEN_SHA256) ? admin : reply("Unauthorized", 401);
  }
  if (url.pathname !== "/v1/tunnel") return reply("Not found", 404);
  if (request.method !== "GET" || request.headers.get("upgrade")?.toLowerCase() !== "websocket" || request.headers.get("sec-websocket-protocol") !== protocol) return reply("WebSocket upgrade required", 400);
  const role = request.headers.get("x-removent-role");
  if ((role !== "host" && role !== "client") || request.headers.get("x-removent-room") !== env.RELAY_ROOM) return reply("Unauthorized", 401);
  const hash = role === "host" ? env.RELAY_HOST_TOKEN_SHA256 : env.RELAY_CLIENT_TOKEN_SHA256;
  const keys = deviceKeys(role === "host" ? env.RELAY_HOST_PUBLIC_KEYS : env.RELAY_CLIENT_PUBLIC_KEYS);
  if (hash ? !await authorized(request, hash) : request.headers.has("authorization")) return reply("Unauthorized", 401);
  return await verifyAdmission(request, role, keys) ? role : reply("Unauthorized", 401);
}

const unhex = (value: string) => Uint8Array.from(value.match(/../g)!, p => parseInt(p, 16));
async function verifyAdmission(request: Request, role: Role, keys: string[]): Promise<boolean> {
  const h = request.headers;
  const key = h.get("x-removent-key") ?? "", nonce = h.get("x-removent-nonce") ?? "";
  const signature = h.get("x-removent-signature") ?? "", time = h.get("x-removent-time") ?? "";
  const audience = h.get("x-removent-audience") ?? "";
  const url = new URL(request.url);
  const expectedAudience = `removent://${url.hostname}:${url.port || (url.protocol === "https:" ? "443" : "80")}`;
  if (audience !== expectedAudience || !hex.test(key) || !hex.test(nonce) || !/^[0-9a-f]{128}$/i.test(signature) || !/^[0-9]{1,12}$/.test(time) ||
      Math.abs(Date.now() / 1000 - Number(time)) > 120 || (keys.length > 0 && !keys.includes(key.toLowerCase()))) return false;
  try {
    const publicKey = await crypto.subtle.importKey("raw", unhex(key), "Ed25519", false, ["verify"]);
    const message = `removent-relay-admission-v1\n${audience}\n${h.get("x-removent-room")}\n${role}\n${time}\n${nonce}`;
    return await crypto.subtle.verify("Ed25519", publicKey, unhex(signature), new TextEncoder().encode(message));
  } catch { return false; }
}

/** Bounded, durable replay ledger; stays below 128 KiB serialized and is
 * claimed atomically before starting a container. */
export function claimAdmission(request: Request, entries: Record<string, number>, now = Date.now() / 1000): Record<string, number> | undefined {
  const key = `${request.headers.get("x-removent-key")}:${request.headers.get("x-removent-nonce")}`;
  const active = Object.fromEntries(Object.entries(entries).filter(([, expiry]) => expiry >= now));
  if (active[key] !== undefined || Object.keys(active).length >= 512) return undefined;
  active[key] = Number(request.headers.get("x-removent-time")) + 121;
  return active;
}

export interface Runtime {
  running(): boolean;
  start(signal: AbortSignal): Promise<void>;
  destroy(): Promise<void>;
  forward(request: Request): Promise<Response>;
}
export interface Storage {
  get(key: string): Promise<boolean | undefined>;
  put(key: string, value: boolean): Promise<void>;
}

/** Explicit stop is durable. Only a controller (or admin start) may wake an
 * enabled-but-idle container; host retries and status requests never start it. */
export class RelayControl {
  private enabled = false;
  private revision = 0;
  private tail: Promise<unknown> = Promise.resolve();
  private queued = 0;
  private starting?: AbortController;
  constructor(private storage: Storage, private runtime: Runtime) {}
  async restore(): Promise<void> { this.enabled = (await this.storage.get("enabled")) === true; }
  status(): Response {
    return Response.json({
      enabled: this.enabled,
      running: this.runtime.running(),
      state: !this.enabled ? "stopped" : this.starting ? "starting" : this.runtime.running() ? "running" : "sleeping",
    }, { headers: { "cache-control": "no-store" } });
  }
  private serial<T>(work: () => Promise<T>): Promise<T> {
    if (this.queued >= 64) return Promise.reject(new Error("Relay control busy"));
    this.queued++;
    const job = this.tail.then(work);
    this.tail = job.catch(() => {}).finally(() => { this.queued--; });
    return job;
  }
  private async ensureStarted(revision: number): Promise<boolean> {
    if (!this.enabled || revision !== this.revision) return false;
    if (!this.runtime.running()) {
      const start = new AbortController();
      this.starting = start;
      try { await this.runtime.start(start.signal); }
      finally { if (this.starting === start) this.starting = undefined; }
    }
    if (!this.enabled || revision !== this.revision) {
      await this.runtime.destroy();
      return false;
    }
    return true;
  }
  async handle(action: Action, request: Request): Promise<Response> {
    if (action === "status") return this.status();
    if (action === "stop") {
      // Close admission immediately, even while an older cold start is pending.
      this.enabled = false;
      this.revision++;
      this.starting?.abort();
      // Store before stopping; a DO restart or failed destroy must remain closed.
      return this.serial(async () => {
        await this.storage.put("enabled", false);
        await this.runtime.destroy();
        return this.status();
      });
    }
    if (action === "start") {
      const revision = ++this.revision;
      this.enabled = true;
      return this.serial(async () => {
        if (revision !== this.revision || !this.enabled) return this.status();
        await this.storage.put("enabled", true);
        if (!await this.ensureStarted(revision)) return this.status();
        return this.status();
      });
    }
    const revision = this.revision;
    return this.serial(async () => {
      if (!this.enabled || revision !== this.revision) return reply("Relay is stopped", 503);
      if (action === "host" && !this.runtime.running()) return reply("Relay is sleeping; waiting for a controller", 503);
      if (action === "client" && !await this.ensureStarted(revision)) return reply("Relay is stopped", 503);
      // The runtime's forward operation MUST NOT auto-start on a stopped socket.
      // A container can exit idle between this check and the HTTP upgrade.
      return this.runtime.forward(request);
    });
  }
}
