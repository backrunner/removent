import { Container } from "@cloudflare/containers";
import { authenticate, ingress, claimAdmission, deviceKeys, resourceLimits, RelayControl, reply, validateConfig, type RelayConfig } from "./control.js";

interface Env extends RelayConfig { RELAY: DurableObjectNamespace<RelayContainer>; }

export class RelayContainer extends Container<Env> {
  defaultPort = 8080;
  pingEndpoint = "localhost/healthz";
  sleepAfter = "5m";
  enableInternet = false;
  private control: RelayControl;
  private restored: Promise<void>;

  constructor(ctx: DurableObjectState<{}>, env: Env) {
    super(ctx, env);
    const idle = validateConfig(env);
    // Disk is ephemeral. All routing credentials are injected again on every
    // boot; native RVP device identities stay on the Macs, never in this image.
    this.envVars = {
      REMOVENT_RELAY_IDLE_SECS: String(idle),
      REMOVENT_RELAY_CONFIG_JSON: JSON.stringify({
        // Source filtering is at the trusted edge. Container sockets see the proxy.
        listen: "0.0.0.0:8080", identity_dir: "/tmp/unused", allowed_cidrs: [],
        ...resourceLimits(env),
        rooms: [{ name: env.RELAY_ROOM, host_token_sha256: env.RELAY_HOST_TOKEN_SHA256 || "", client_token_sha256: env.RELAY_CLIENT_TOKEN_SHA256 || "",
          host_public_keys: deviceKeys(env.RELAY_HOST_PUBLIC_KEYS), client_public_keys: deviceKeys(env.RELAY_CLIENT_PUBLIC_KEYS) }],
      }),
    };
    this.control = new RelayControl({
      get: key => ctx.storage.get<boolean>(key),
      put: (key, value) => ctx.storage.put(key, value),
    }, {
      running: () => ctx.container?.running ?? false,
      start: signal => this.startAndWaitForPorts({
        ports: [8080], cancellationOptions: { abort: signal, instanceGetTimeoutMS: 10_000, portReadyTimeoutMS: 20_000 },
      }),
      destroy: async () => { if (ctx.container?.running) await this.destroy(); },
      forward: request => {
        // Use the documented low-level TCP-port API to preserve the 101 upgrade
        // without Container.fetch's implicit restart or a JS per-packet proxy.
        if (!ctx.container?.running) return Promise.resolve(reply("Relay is sleeping", 503));
        return ctx.container.getTcpPort(8080).fetch(new Request("http://container/v1/tunnel", request));
      },
    });
    this.restored = ctx.blockConcurrencyWhile(() => this.control.restore());
  }

  override async onActivityExpired(): Promise<void> {
    // Rust owns the controller-aware idle deadline. A permanently connected
    // host must not prevent sleep; an active controller must not be cut off by
    // an HTTP timer that cannot see bytes in the opaque upgraded connection.
    this.renewActivityTimeout();
  }

  override async fetch(request: Request): Promise<Response> {
    await this.restored;
    try {
      const action = await authenticate(request, this.env);
      if (action instanceof Response) return action;
      if (action === "host" || action === "client") {
        const claimed = await this.ctx.storage.transaction(async storage => {
          const next = claimAdmission(request, await storage.get<Record<string, number>>("admission-nonces") ?? {});
          if (!next) return false;
          await storage.put("admission-nonces", next);
          return true;
        });
        if (!claimed) return reply("Replayed admission or relay busy", 401);
      }
      return await this.control.handle(action, request);
    } catch {
      return reply("Relay is temporarily unavailable", 503);
    }
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    // Obtain the stable DO stub only after source and device authentication.
    return ingress(request, env, () => env.RELAY.getByName("relay").fetch(request));
  },
} satisfies ExportedHandler<Env>;
