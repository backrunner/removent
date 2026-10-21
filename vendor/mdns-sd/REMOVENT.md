# Removent mDNS memory fixes

Base: the crates.io `mdns-sd` 0.21.5 source archive, SHA-256
`127d3355da0615643c88c885e29a1ccb29ca4b1ca1f2e44b3caed2e728a796aa`.
Upstream: https://github.com/keepsimple1/mdns-sd/tree/v0.21.5.
The upstream MIT and Apache-2.0 licenses are preserved.

Local changes:

- `service_daemon.rs`: after conflict handling, publisher-only daemons skip
  response caching unless unsolicited caching was explicitly enabled.
- `dns_cache.rs`: expire orphan SRV/TXT and NSEC records independently of PTR
  traversal; remove empty keys and stale subtype mappings; shrink sparse maps
  after churn while preserving normal service-removal notifications.
- `dns_cache_memory_tests.rs` and `service_daemon_memory_tests.rs`: deterministic
  regression coverage for expiration, retained capacity, cache consumers and
  publisher name-conflict handling.
- Cache records and cache-flush updates are scoped to their receiving interface.
  Removal, expiration and expired responses keep a service reachable on another
  interface; cached discovery notifications and follow-up queries are deduplicated.
- `dns_cache_network_tests.rs` and daemon tests cover redundant interfaces,
  independent lifetimes, cache-flush isolation, bounded refreshes and expired
  responses. Known-answer query packets include only records from their outgoing
  interface, so a fresh answer on one link does not suppress another link's refresh.
  Hostname expiry, interface recovery and refresh-counter fixtures use
  one loopback interface and a private port, with bounded waits and cleanup.
  The recovery fixture waits for both startup announcements before removing its
  interface, so it does not race the initial announcement's multicast rate limit.
- Custom-port daemons use their configured port to identify mDNS queries, keeping
  normal multicast and legacy unicast response paths distinct.
- Integration fixtures count operational interfaces, isolate IPv4/IPv6 multicast
  loop settings to their respective family, and wait for asynchronous SRV updates
  before asserting the converged cache. Hostname scope checks select one interface.
  The parser and changelog have whitespace-only normalization.

The root Cargo patch applies to every consumer, including the daemon, desktop
and mobile client. The dependency is pinned so `cargo update` cannot silently
bypass the fix. `cargo test --manifest-path vendor/mdns-sd/Cargo.toml --locked -- --test-threads=1`
runs the complete library, integration and documentation suites, including local regressions; CI runs this
explicitly because dependencies' own unit tests are not part of
`cargo test --workspace`.

`cargo run --manifest-path vendor/mdns-sd/Cargo.toml --locked --example memory_probe`
checks five rounds of real loopback UDP traffic on a private port. It validates
that a publisher retains no response records, a cache consumer drops expired
records, and both return to one background timer. On macOS it also reports the
process physical footprint. CI runs this probe as well.

Remove this patch once an upstream release passes the same regressions.
