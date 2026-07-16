# Callback SSRF: resolve-once + address pinning

**Status:** implemented (branch `sec/callback-ssrf-pinning`).
**Findings closed:** GED-SEC-08 (HIGH), GED-SEC-09 (MEDIUM), GED-SEC-10 (re-triaged).
**Report:** `docs/security/pentest-2026-07-15.md`.

## Problem

`CallbackUrlPolicy` guarded the async-upload `callback_url` against SSRF into the
internal network, but the barrier had two structural holes the 2026-07-15 pentest
turned into a complete, end-to-end SSRF to loopback:

1. **Numeric IP encodings (GED-SEC-08).** `IPAddr.new` rejects `2130706433`
   (decimal), `0x7f000001` (hex), `0177.0.0.1` (octal) and `127.1` (short form),
   so `ip_literal` returned `nil` and they fell through to the DNS branch. The
   old `Resolv.getaddresses` returned `[]` for most of them (the "unresolvable ⇒
   allow" branch, GED-SEC-10), and then `Net::HTTP` connected through **libc's
   `getaddrinfo`**, which *does* parse those forms — straight to `127.0.0.1`.
   The request-time 422 never fired; the delivery-time barrier was *guaranteed to
   allow*.

2. **Resolve/connect TOCTOU (GED-SEC-09).** The policy resolved the host once to
   decide "safe?", then `DeliverCallbackJob` opened the socket with a *second,
   independent* libc resolution. An attacker controlling DNS answers the check
   with a public IP and the connect with a private one (DNS rebinding).

3. **Platform divergence.** `0177.0.0.1` is a *public* address on Darwin's libc
   (`177.0.0.1`) but *loopback* on glibc (the production image). A fix that just
   switched the policy to libc resolution would go green on the laptop and stay
   exploitable in production.

## Decision

**Resolve the callback host exactly once, with the same resolver the socket uses,
then pin the approved address so libc never re-parses the hostile string.**

### `CallbackUrlPolicy`

- `reject_reason(url, resolve:)` stays a thin wrapper. A new
  `decide(url, resolve:) → Decision(reason:, ipaddr:)` carries both the rejection
  reason (or `nil`) and the vetted address to pin (or `nil`).
- **`host_ip(host) = ip_literal(host) || inet_aton(host)`**, evaluated **outside
  the `resolve` flag** (pure string parsing, no DNS). `inet_aton` is a
  self-implemented normalizer for the numeric IPv4 forms libc accepts but
  `IPAddr` rejects: 1–4 dot-separated parts, each in decimal / hex (`0x…`) /
  octal (`0…`), the last part absorbing the remaining low bytes. All of
  `2130706433`, `0x7f000001`, `0177.0.0.1`, `127.1` normalize to `127.0.0.1` and
  are blocked at request time. Implementing it ourselves (not trusting libc) is
  what neutralizes the Darwin/glibc octal divergence: the string is resolved to a
  canonical integer address *before* any resolver sees it.
- Real hostnames resolve via **`Addrinfo.getaddrinfo(host, nil, nil, :STREAM)`**
  — the same libc path `Net::HTTP` connects through — instead of the pure-Ruby
  `Resolv`, so the address vetted is the address the socket would use.
- **Unresolvable ⇒ refuse** (`UNRESOLVABLE_MESSAGE`), reversing the old
  GED-SEC-10 "allow" branch. Under pinning the question dissolves: no address
  means nothing to pin, so the job cannot connect by construction. Allowing it
  would force a hostname fallback (a second resolution) — reopening GED-SEC-09
  for exactly the inputs GED-SEC-08 exploits.

### `DeliverCallbackJob`

- Call `CallbackUrlPolicy.decide(callback_url, resolve: true)` once. On any
  `reason`, refuse (no `DeliveryFailed`, no retry — a forbidden destination must
  not be retried).
- Connect with `Net::HTTP.new(uri.hostname, uri.port)` and set
  `http.ipaddr = decision.ipaddr` **before `start`**. Pinning the IP keeps the
  original hostname for the `Host` header, TLS SNI and certificate validation
  while forcing the socket to the vetted address — no re-resolution, no rebinding
  window.
- Use `uri.hostname` (not `uri.host`) consistently, fixing the pre-existing
  bracketed-IPv6 check/connect inconsistency.

## Consequences / tradeoffs

- **An unresolvable callback host is now rejected**, where it used to be silently
  accepted-then-fail. Behavioral change for clients pointing at a host that does
  not resolve; that was never a working delivery anyway.
- **Tests must stub `Addrinfo.getaddrinfo`, not `Resolv`.** `DnsTestHelpers` was
  rewritten to swap the `Addrinfo.getaddrinfo` singleton (returning objects that
  respond to `#ip_address`) and gained a non-block `stub_dns!`/`unstub_dns!`
  variant for setup/teardown defaults. Any test that drives real callback
  delivery needs a default public resolution for its callback host, or the
  "unresolvable ⇒ refuse" rule blocks the POST (`deliver_callback_job_test.rb`,
  `async_upload_test.rb`).
- **Brakeman** flags `Net::HTTP.new(<model-derived hostname>, …)` as
  "Model attribute used in file name" (Weak confidence, `FileAccess`). It is a
  false positive — a network connect, not a filesystem path — and the host is
  SSRF-vetted + pinned; ignored with a note in `config/brakeman.ignore`.
- **Linux verification is mandatory** for this change: the `0177.0.0.1`
  Darwin/glibc split means a Darwin-green run does not prove glibc behavior, so
  `test/security/ssrf_test.rb` is also run inside the Docker (glibc) image.

## Non-goals

- IPv6 numeric-encoding oddities beyond what `IPAddr` already parses — the
  `inet_aton` normalizer is IPv4-only, matching the attack surface libc exposes.
- Blocking at the socket layer (a custom resolver / `TCPSocket` guard) — pinning
  the already-vetted address achieves the same guarantee with far less surface.
