import { test } from "node:test";
import assert from "node:assert/strict";
import { allowsIP, allowedSource, networks } from "../src/network.js";
import { resourceLimits, type RelayConfig } from "../src/control.js";

const env = { RELAY_ALLOWED_CIDRS: "203.0.113.0/24,2001:db8::/32" } as RelayConfig;
function request(ip?: string, path = "/v1/tunnel") {
  const req = new Request(`https://relay.example${path}`, { headers: ip ? { "cf-connecting-ip": ip } : {} });
  Object.defineProperty(req, "cf", { value: { colo: "TEST" } });
  return req;
}

test("CIDRs cover IPv4, IPv6 and mapped peers with exact prefix boundaries", () => {
  for (const ip of ["203.0.113.0", "203.0.113.255", "::ffff:203.0.113.4", "2001:db8:ffff::1"]) assert.ok(allowsIP(ip, env.RELAY_ALLOWED_CIDRS), ip);
  for (const ip of ["203.0.112.255", "203.0.114.0", "2001:db9::1", "2001:db8::1%en0", "203.0.113.4,1.2.3.4", "0xcb007104", "[2001:db8::1]", "", null]) assert.equal(allowsIP(ip, env.RELAY_ALLOWED_CIDRS), false, String(ip));
  assert.ok(allowsIP("203.0.113.4", "::ffff:203.0.113.0/120"));
  assert.equal(allowsIP("203.0.113.5", "203.0.113.4/32"), false);
  assert.ok(allowsIP("2001:db8::1", "2001:db8::1/128"));
  assert.equal(allowsIP("2001:db8::2", "2001:db8::1/128"), false);
  assert.ok(allowsIP("1.2.3.4", "0.0.0.0/0"));
  assert.equal(allowsIP("::1", "0.0.0.0/0"), false);
  assert.ok(allowsIP(null, ""), "empty means unrestricted");
  for (const invalid of ["1.2.3.4", "0.0.0.0/33", "::/129", "::ffff:1.2.3.4/95", "::1/-1", "::1/01", "127.1/8", "010.0.0.0/8", "1.2.3.4/32,", Array(257).fill("::/0").join(",")]) assert.throws(() => networks(invalid), invalid);
});

test("only trusted edge source can pass; spoofed forwarding headers do not grant access", () => {
  assert.ok(allowedSource(request("203.0.113.4"), env));
  const denied = request("198.51.100.1");
  denied.headers.set("x-forwarded-for", "203.0.113.4");
  denied.headers.set("x-real-ip", "203.0.113.4");
  assert.equal(allowedSource(denied, env), false);
  assert.equal(allowedSource(request(), env), false);
  assert.equal(allowedSource(new Request("https://relay.example", { headers: { "cf-connecting-ip": "203.0.113.4" } }), env), false);
  const subrequest = request("203.0.113.4"); subrequest.headers.set("cf-worker", "attacker.example");
  assert.equal(allowedSource(subrequest, env), false);
  const pseudo = request("240.0.0.1"); pseudo.headers.set("cf-connecting-ipv6", "2001:db8::1");
  assert.equal(allowedSource(pseudo, env), false, "extra IPv6 header cannot bypass source policy");
});

test("admin policy inherits by default and can be restricted independently", () => {
  assert.ok(allowedSource(request("203.0.113.4", "/admin/stop"), env));
  const restricted = { ...env, RELAY_ADMIN_ALLOWED_CIDRS: "198.51.100.1/32" };
  assert.equal(allowedSource(request("203.0.113.4", "/admin/start"), restricted), false);
  assert.ok(allowedSource(request("198.51.100.1", "/admin/status"), restricted));
  assert.equal(allowedSource(request("198.51.100.1"), restricted), false);
  assert.ok(allowedSource(request("198.51.100.1", "/admin/start"), { ...env, RELAY_ADMIN_ALLOWED_CIDRS: "" }));
  assert.equal(resourceLimits({ ...env, RELAY_MAX_CONNECTIONS: "12" }).max_connections, 12);
  assert.throws(() => resourceLimits({ ...env, RELAY_MAX_CONNECTIONS: "0" }));
});
