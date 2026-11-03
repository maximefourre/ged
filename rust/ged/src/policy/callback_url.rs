//! SSRF guard for client-supplied callback URLs: the async upload callback
//! must never POST into the internal network. Port of
//! `app/services/callback_url_policy.rb`. Checked twice with the same rules —
//! syntactically at request time ([`reject_reason`], no DNS, a fast 422) and
//! with DNS resolution at delivery time ([`decide`] with `resolve: true`, the
//! effective boundary).
//!
//! Numeric hosts are normalised **here** (`inet_aton`), never by the system
//! resolver: `0177.0.0.1` is public on Darwin's libc and loopback on glibc, so
//! trusting libc would pass on a laptop and stay exploitable in production.
//!
//! For a real hostname, [`decide`] resolves **once** through the same libc
//! path `Net::HTTP`/`reqwest` use (`getaddrinfo`) and returns the approved
//! address in [`Decision::ipaddr`]; the delivery job pins that address when
//! connecting (keeping Host/SNI), so there is no resolve/connect window. The
//! pinning itself is the job's (T9); this module only produces the address.
//! An unresolvable host is refused: no address means nothing to pin.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const INVALID_MESSAGE: &str = "must be an http(s) URL with a host";
pub const BLOCKED_MESSAGE: &str = "must not target a private or internal address";
pub const UNRESOLVABLE_MESSAGE: &str = "must resolve to a routable address";

/// Why a URL was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not an http(s) URL with a host.
    Invalid,
    /// Targets a private or internal address.
    Blocked,
    /// A hostname that resolves to nothing (nothing to pin).
    Unresolvable,
}

impl Refusal {
    /// The message Rails adds to `errors[:callback_url]`.
    pub fn message(self) -> &'static str {
        match self {
            Self::Invalid => INVALID_MESSAGE,
            Self::Blocked => BLOCKED_MESSAGE,
            Self::Unresolvable => UNRESOLVABLE_MESSAGE,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Refusal {}

/// An accepted URL. `ipaddr` is the address to pin at connect time: always set
/// for a numeric host and, with `resolve: true`, for a resolved hostname. It
/// is `None` only for a hostname that was not resolved (request-time check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub ipaddr: Option<IpAddr>,
}

/// Request-time check: `None` when acceptable, else the rejection message.
/// Never touches DNS.
pub fn reject_reason(url: &str) -> Option<&'static str> {
    decide_syntactic(url).err().map(Refusal::message)
}

/// Syntactic decision (`resolve: false`): no DNS.
pub fn decide_syntactic(url: &str) -> Result<Decision, Refusal> {
    match classify(url)? {
        Target::Ip(ip) => Ok(Decision { ipaddr: Some(ip) }),
        Target::Name(_) => Ok(Decision { ipaddr: None }),
    }
}

/// Full decision. With `resolve: true` a hostname is resolved once through the
/// system resolver and the first address is returned for pinning.
pub async fn decide(url: &str, resolve: bool) -> Result<Decision, Refusal> {
    decide_with(url, resolve, system_resolve).await
}

/// [`decide`] with an injectable resolver (tests).
pub async fn decide_with<F, Fut>(url: &str, resolve: bool, resolver: F) -> Result<Decision, Refusal>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Vec<IpAddr>>,
{
    let host = match classify(url)? {
        Target::Ip(ip) => return Ok(Decision { ipaddr: Some(ip) }),
        Target::Name(host) => host,
    };
    // Real hostname: only touch DNS when asked to (delivery time).
    if !resolve {
        return Ok(Decision { ipaddr: None });
    }
    let addresses = resolver(host).await;
    let Some(first) = addresses.first().copied() else {
        return Err(Refusal::Unresolvable);
    };
    if addresses.iter().any(|ip| is_blocked(*ip)) {
        return Err(Refusal::Blocked);
    }
    // Pin the first approved address so the connect uses exactly what we vetted.
    Ok(Decision {
        ipaddr: Some(first),
    })
}

/// `getaddrinfo`, the libc path the HTTP client connects with. Empty on failure.
async fn system_resolve(host: String) -> Vec<IpAddr> {
    match tokio::net::lookup_host((host.as_str(), 0)).await {
        Ok(addrs) => addrs.map(|a| a.ip()).collect(),
        Err(_) => Vec::new(),
    }
}

enum Target {
    Ip(IpAddr),
    Name(String),
}

/// URL → numeric address (blocked ones refused here) or hostname.
fn classify(url: &str) -> Result<Target, Refusal> {
    let host = parse_http_host(url).ok_or(Refusal::Invalid)?;
    // A host that is itself an address (no DNS): an IP literal, or a numeric
    // IPv4 encoding that libc would accept. Kept outside the resolve flag so
    // the request-time check blocks them too.
    if let Some(ip) = host_ip(&host) {
        if is_blocked(ip) {
            return Err(Refusal::Blocked);
        }
        return Ok(Target::Ip(ip));
    }
    Ok(Target::Name(host))
}

fn host_ip(host: &str) -> Option<IpAddr> {
    ip_literal(host).or_else(|| inet_aton(host).map(IpAddr::V4))
}

/// `IPAddr.new(host)`: strict dotted-quad IPv4 (no leading zeros) or IPv6.
fn ip_literal(host: &str) -> Option<IpAddr> {
    host.parse::<IpAddr>().ok()
}

/// Self-implemented `inet_aton`: 1-4 dot-separated parts in decimal, hex
/// (`0x…`) or octal (`0…`), the last part absorbing the remaining low bytes, so
/// `2130706433`, `0x7f000001`, `0177.0.0.1` and `127.1` all become `127.0.0.1`.
fn inet_aton(host: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let nums: Vec<u64> = parts
        .iter()
        .map(|p| parse_uint(p))
        .collect::<Option<Vec<_>>>()?;
    let (last, head) = nums.split_last()?;
    if head.iter().any(|n| *n > 255) {
        return None;
    }
    // 256 ** (5 - parts)
    if *last >= 1u64 << (8 * (5 - parts.len())) {
        return None;
    }
    let mut value: u64 = *last;
    for (i, n) in head.iter().enumerate() {
        value += n << (8 * (3 - i));
    }
    u32::try_from(value).ok().map(Ipv4Addr::from)
}

/// One `inet_aton` part: unsigned, decimal, `0x`-hex or `0`-octal. Anything
/// else (a DNS label such as `com`, an overflow) is `None`.
fn parse_uint(part: &str) -> Option<u64> {
    let bytes = part.as_bytes();
    let (digits, radix) = match bytes {
        [b'0', b'x' | b'X', rest @ ..] if !rest.is_empty() => (rest, 16),
        [b'0'] => return Some(0),
        [b'0', rest @ ..] => (rest, 8),
        [b'1'..=b'9', ..] => (bytes, 10),
        _ => return None,
    };
    if !digits.iter().all(|b| (*b as char).is_digit(radix)) {
        return None;
    }
    u64::from_str_radix(std::str::from_utf8(digits).ok()?, radix).ok()
}

const BLOCKED_V4: [(Ipv4Addr, u8); 7] = [
    (Ipv4Addr::new(0, 0, 0, 0), 8),
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(127, 0, 0, 0), 8),
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
];

/// `BLOCKED_RANGES`, with `::ffff:a.b.c.d` judged as `a.b.c.d`.
pub fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => blocked_v4(v4),
            None => blocked_v6(v6),
        },
    }
}

fn blocked_v4(ip: Ipv4Addr) -> bool {
    let value = u32::from(ip);
    BLOCKED_V4.iter().any(|(net, bits)| {
        let mask = u32::MAX << (32 - u32::from(*bits));
        value & mask == u32::from(*net) & mask
    })
}

fn blocked_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip == Ipv6Addr::LOCALHOST
        || first & 0xfe00 == 0xfc00 // fc00::/7
        || first & 0xffc0 == 0xfe80 // fe80::/10
}

// --- URL parsing (RFC 3986, as Ruby's URI.parse) ----------------------------

/// The hostname of a valid `http(s)` URL with a non-empty host (`uri.hostname`:
/// IPv6 literals unbracketed), or `None` when Ruby's `URI.parse` would raise
/// or the URL is not an HTTP one.
fn parse_http_host(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once(':')?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return None;
    }
    let rest = rest.strip_prefix("//")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    if !valid_tail(tail) {
        return None;
    }
    let hostport = match authority.split_once('@') {
        Some((userinfo, hostport)) => {
            if !userinfo
                .bytes()
                .all(|b| is_unreserved(b) || is_sub_delim(b) || b == b':')
                || !valid_pct(userinfo)
            {
                return None;
            }
            hostport
        }
        None => authority,
    };
    let host = if let Some(inner) = hostport.strip_prefix('[') {
        let (inner, after) = inner.split_once(']')?;
        valid_port(after)?;
        if let Some(future) = inner.strip_prefix(['v', 'V']) {
            // IPvFuture: syntactically valid, never an address we can judge.
            let (hex, tail) = future.split_once('.')?;
            let ok = !hex.is_empty()
                && hex.bytes().all(|b| b.is_ascii_hexdigit())
                && !tail.is_empty()
                && tail
                    .bytes()
                    .all(|b| is_unreserved(b) || is_sub_delim(b) || b == b':');
            if !ok {
                return None;
            }
        } else {
            inner.parse::<Ipv6Addr>().ok()?;
        }
        inner.to_owned()
    } else {
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        };
        if let Some(port) = port
            && !port.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        if !host
            .bytes()
            .all(|b| is_unreserved(b) || is_sub_delim(b) || b == b'%')
            || !valid_pct(host)
        {
            return None;
        }
        host.to_owned()
    };
    (!host.is_empty()).then_some(host)
}

/// `after` is what follows `]`: empty, or `:` digits.
fn valid_port(after: &str) -> Option<()> {
    match after.strip_prefix(':') {
        None if after.is_empty() => Some(()),
        Some(port) if port.bytes().all(|b| b.is_ascii_digit()) => Some(()),
        _ => None,
    }
}

/// Path, query and fragment: pchar / `/` / `?`, with at most one `#`.
fn valid_tail(tail: &str) -> bool {
    let (before_fragment, fragment) = match tail.split_once('#') {
        Some((a, b)) => (a, Some(b)),
        None => (tail, None),
    };
    let ok = |part: &str| {
        part.bytes().all(|b| {
            is_unreserved(b) || is_sub_delim(b) || matches!(b, b':' | b'@' | b'/' | b'?' | b'%')
        }) && valid_pct(part)
    };
    ok(before_fragment) && fragment.is_none_or(ok)
}

/// Every `%` starts a `%HH`.
fn valid_pct(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().all(|(i, b)| {
        *b != b'%'
            || (bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit))
    })
}

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

fn is_sub_delim(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    async fn with_dns(url: &str, addresses: &[&str]) -> Result<Decision, Refusal> {
        let addresses: Vec<IpAddr> = addresses.iter().map(|a| ip(a)).collect();
        decide_with(url, true, move |_| async move { addresses }).await
    }

    #[test]
    fn accepts_a_public_hostname_and_a_public_ip_literal_without_resolving() {
        assert_eq!(reject_reason("https://example.com/cb"), None);
        assert_eq!(reject_reason("http://93.184.216.34/cb"), None);
        assert_eq!(
            decide_syntactic("http://93.184.216.34/cb").unwrap().ipaddr,
            Some(ip("93.184.216.34"))
        );
        assert_eq!(
            decide_syntactic("https://example.com/cb").unwrap().ipaddr,
            None
        );
    }

    #[test]
    fn rejects_non_http_or_hostless_urls() {
        for url in [
            "ftp://example.com/cb",
            "not a url",
            "http://",
            "javascript:alert(1)",
            "http:///path",
            "",
            "http://exa mple.com/",
            "http://example.com/a b",
            "http://example.com:80x/",
            "http://a@b@c/",
            "http://example.com/%zz",
            "//example.com/cb",
            "http://[::1/cb",
            "http://[zz]/cb",
        ] {
            assert_eq!(reject_reason(url), Some(INVALID_MESSAGE), "{url}");
        }
    }

    #[test]
    fn the_scheme_is_case_insensitive_and_userinfo_and_port_are_fine() {
        assert_eq!(
            reject_reason("HTTPS://user:pw@example.com:8443/cb?a=1#f"),
            None
        );
    }

    #[test]
    fn rejects_blocked_ip_literals_without_touching_dns() {
        for url in [
            "http://127.0.0.1/cb",
            "http://10.0.0.5/cb",
            "http://172.16.3.4/cb",
            "http://192.168.1.1/cb",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/cb",
            "http://0.0.0.0/cb",
            "http://[::1]/cb",
            "http://[fe80::1]/cb",
            "http://[fc00::1]/cb",
            "http://[::ffff:127.0.0.1]/cb",
            "http://[::1]:8080/cb",
        ] {
            assert_eq!(reject_reason(url), Some(BLOCKED_MESSAGE), "{url}");
        }
    }

    #[test]
    fn range_edges() {
        for blocked in [
            "100.127.255.255",
            "172.31.255.255",
            "127.255.255.255",
            "0.255.255.255",
        ] {
            assert!(is_blocked(ip(blocked)), "{blocked}");
        }
        for open in [
            "100.128.0.0",
            "172.32.0.0",
            "172.15.255.255",
            "1.0.0.0",
            "169.255.0.0",
            "2001:4860:4860::8888",
            "::ffff:93.184.216.34",
        ] {
            assert!(!is_blocked(ip(open)), "{open}");
        }
    }

    /// The four numeric loopback encodings of the SSRF audit (GED-SEC-08).
    #[test]
    fn numeric_ipv4_encodings_are_normalised_by_inet_aton() {
        for host in [
            "2130706433",
            "0x7f000001",
            "0X7F000001",
            "0177.0.0.1",
            "127.1",
            "127.0.1",
            "0x7f.1",
            "017700000001",
        ] {
            assert_eq!(inet_aton(host), Some(Ipv4Addr::LOCALHOST), "{host}");
            assert_eq!(
                reject_reason(&format!("http://{host}/cb")),
                Some(BLOCKED_MESSAGE),
                "{host}"
            );
        }
        assert_eq!(inet_aton("0"), Some(Ipv4Addr::UNSPECIFIED));
        assert_eq!(inet_aton("0x0"), Some(Ipv4Addr::UNSPECIFIED));
        assert_eq!(inet_aton("1.2.3.4"), Some(Ipv4Addr::new(1, 2, 3, 4)));
        // a public address in an odd encoding stays public
        assert_eq!(
            inet_aton("0x5db8d822"),
            Some(Ipv4Addr::new(93, 184, 216, 34))
        );
        assert_eq!(reject_reason("http://0x5db8d822/cb"), None);
    }

    #[test]
    fn what_is_not_numeric_is_left_to_dns() {
        for host in [
            "com",
            "a.b",
            "1.2.3.4.5",
            "1..2",
            "256.1.1.1",
            "999.1.1.1",
            "0xg",
            "08",
            "1.2.3.256",
            "4294967296",
            "0x100000000",
            "1.16777216",
            "99999999999999999999999",
            "127.0.0.1.",
            "-1",
        ] {
            assert_eq!(inet_aton(host), None, "{host}");
        }
    }

    /// `999.1.1.1` is not an address of any encoding: request time lets it
    /// through as a hostname (as Rails does), and delivery refuses it because
    /// it does not resolve — it can never reach the network.
    #[tokio::test]
    async fn an_impossible_dotted_quad_is_a_name_that_must_resolve() {
        assert_eq!(reject_reason("http://999.1.1.1/cb"), None);
        assert_eq!(
            decide_syntactic("http://999.1.1.1/cb").unwrap().ipaddr,
            None
        );
        assert_eq!(
            with_dns("http://999.1.1.1/cb", &[]).await,
            Err(Refusal::Unresolvable)
        );
        // and for real: getaddrinfo cannot resolve it
        assert_eq!(
            decide("http://999.1.1.1/cb", true).await,
            Err(Refusal::Unresolvable)
        );
    }

    #[tokio::test]
    async fn a_bracketed_ipv6_loopback_is_blocked_with_and_without_resolve() {
        assert_eq!(
            decide("http://[::1]/cb", false).await,
            Err(Refusal::Blocked)
        );
        assert_eq!(decide("http://[::1]/cb", true).await, Err(Refusal::Blocked));
        assert_eq!(
            decide("http://[2001:4860:4860::8888]/cb", true).await,
            Ok(Decision {
                ipaddr: Some(ip("2001:4860:4860::8888"))
            })
        );
    }

    #[tokio::test]
    async fn resolve_false_never_queries_dns() {
        let result = decide_with("http://caller.test/cb", false, |_| async {
            panic!("DNS was queried")
        })
        .await;
        assert_eq!(result, Ok(Decision { ipaddr: None }));
    }

    #[tokio::test]
    async fn resolve_true_rejects_a_hostname_resolving_to_a_blocked_address() {
        assert_eq!(
            with_dns("http://evil.test/cb", &["10.0.0.5"]).await,
            Err(Refusal::Blocked)
        );
    }

    #[tokio::test]
    async fn resolve_true_rejects_when_any_resolved_address_is_blocked() {
        assert_eq!(
            with_dns("http://evil.test/cb", &["93.184.216.34", "127.0.0.1"]).await,
            Err(Refusal::Blocked)
        );
        assert_eq!(
            with_dns("http://evil.test/cb", &["93.184.216.34", "::ffff:10.0.0.1"]).await,
            Err(Refusal::Blocked)
        );
    }

    #[tokio::test]
    async fn resolve_true_accepts_and_returns_the_first_public_address_to_pin() {
        assert_eq!(
            with_dns("http://caller.test/cb", &["93.184.216.34", "93.184.216.35"]).await,
            Ok(Decision {
                ipaddr: Some(ip("93.184.216.34"))
            })
        );
    }

    #[tokio::test]
    async fn resolve_true_refuses_an_unresolvable_host() {
        assert_eq!(
            with_dns("http://nowhere.test/cb", &[]).await,
            Err(Refusal::Unresolvable)
        );
        assert_eq!(
            Refusal::Unresolvable.message(),
            "must resolve to a routable address"
        );
    }

    /// The resolver is handed the bare host (brackets and port gone).
    #[tokio::test]
    async fn the_resolver_receives_the_hostname_only() {
        let seen = std::sync::Mutex::new(String::new());
        let _ = decide_with("https://u:p@Caller.Test:8443/cb?x#y", true, |host| {
            *seen.lock().unwrap() = host;
            async { vec![ip("93.184.216.34")] }
        })
        .await;
        assert_eq!(*seen.lock().unwrap(), "Caller.Test");
    }
}
