import ipaddr from "ipaddr.js";
import type { RelayConfig } from "./control.js";

type Range = [ipaddr.IPv4 | ipaddr.IPv6, number];
const cache = new Map<string, Range[]>();

function address(value: string): ipaddr.IPv4 | ipaddr.IPv6 {
  if (!value || /[%\[\]\s/]/.test(value)) throw new Error("Invalid IP address");
  // Reject nonstandard IPv4 forms (octal, hex, shortened addresses).
  const dotted = value.includes(":") ? value.slice(value.lastIndexOf(":") + 1) : value;
  if (dotted.includes(".") || !value.includes(":")) {
    if (!/^(0|[1-9][0-9]{0,2})(\.(0|[1-9][0-9]{0,2})){3}$/.test(dotted)) throw new Error("Invalid IPv4 address");
  }
  const parsed = ipaddr.parse(value);
  return parsed;
}

export function networks(value = ""): Range[] {
  if (cache.has(value)) return cache.get(value)!;
  const parts = value.trim() ? value.split(",").map(s => s.trim()) : [];
  if (parts.length > 256) throw new Error("At most 256 allowed CIDRs");
  const ranges: Range[] = parts.map(part => {
    const pieces = part.split("/");
    if (pieces.length !== 2 || !/^(0|[1-9][0-9]{0,2})$/.test(pieces[1]!)) throw new Error("Expected IP/prefix CIDR");
    let ip = address(pieces[0]!);
    let prefix = Number(pieces[1]);
    if (prefix > (ip.kind() === "ipv4" ? 32 : 128)) throw new Error("Invalid CIDR prefix");
    if (ip instanceof ipaddr.IPv6 && ip.isIPv4MappedAddress()) {
      if (prefix < 96) throw new Error("Mapped IPv4 CIDR requires prefix >= 96");
      ip = ip.toIPv4Address(); prefix -= 96;
    }
    return [ip, prefix];
  });
  if (cache.size >= 8) cache.clear();
  cache.set(value, ranges);
  return ranges;
}

export function allowsIP(value: string | null, cidrs = ""): boolean {
  const ranges = networks(cidrs);
  if (!ranges.length) return true;
  if (!value) return false;
  try {
    let ip = address(value);
    if (ip instanceof ipaddr.IPv6 && ip.isIPv4MappedAddress()) ip = ip.toIPv4Address();
    return ranges.some(([network, prefix]) => ip.kind() === network.kind() && ip.match(network, prefix));
  } catch { return false; }
}

/** Called only at the public Worker ingress, before accessing the DO. The
 * platform supplies CF-Connecting-IP; never use X-Forwarded-For/X-Real-IP.
 * Worker subrequests can rewrite client identity, so restricted ingress rejects
 * them. DOs are reachable only via this binding, not a public URL. */
export function allowedSource(request: Request, env: RelayConfig): boolean {
  const list = new URL(request.url).pathname.startsWith("/admin/")
    ? (env.RELAY_ADMIN_ALLOWED_CIDRS ?? env.RELAY_ALLOWED_CIDRS)
    : env.RELAY_ALLOWED_CIDRS;
  if (!networks(list).length) return true;
  if (!(request as Request & { cf?: unknown }).cf || request.headers.has("cf-worker")) return false;
  // Require native CF-Connecting-IP (Pseudo IPv4 overwrite must be off).
  // Other IP headers may be client supplied when that optional feature is off.
  return allowsIP(request.headers.get("cf-connecting-ip"), list);
}
