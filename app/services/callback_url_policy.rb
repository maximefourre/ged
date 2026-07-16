require "ipaddr"
require "socket"

# SSRF guard for client-supplied callback URLs: the async upload callback must
# never POST into the internal network. Checked twice with the same rules —
# syntactically at request time (resolve: false, catches IP literals and numeric
# IP encodings with a fast 422) and with DNS resolution at delivery time in
# DeliverCallbackJob (resolve: true, the effective boundary).
#
# Resolution and pinning: at delivery time we resolve the host ONCE via
# Addrinfo.getaddrinfo — the same libc resolver Net::HTTP uses — and return the
# approved address in Decision#ipaddr so the job can PIN it (Net::HTTP#ipaddr=).
# libc therefore never re-parses the hostile string, closing the resolve/connect
# TOCTOU (GED-SEC-09) and the numeric-encoding + platform-divergence bypass
# (GED-SEC-08). An unresolvable host is refused (GED-SEC-10): no address means
# nothing to pin, so the job cannot connect by construction.
class CallbackUrlPolicy
  BLOCKED_RANGES = %w[
    0.0.0.0/8
    10.0.0.0/8
    100.64.0.0/10
    127.0.0.0/8
    169.254.0.0/16
    172.16.0.0/12
    192.168.0.0/16
    ::1/128
    fc00::/7
    fe80::/10
  ].map { |cidr| IPAddr.new(cidr) }.freeze

  INVALID_MESSAGE = "must be an http(s) URL with a host".freeze
  BLOCKED_MESSAGE = "must not target a private or internal address".freeze
  UNRESOLVABLE_MESSAGE = "must resolve to a routable address".freeze

  # reason: nil when acceptable, else the rejection message.
  # ipaddr:  the approved address to pin at connect time (String), or nil.
  Decision = Data.define(:reason, :ipaddr)

  class << self
    # Returns nil when the URL is acceptable, or the reason it is not.
    # Thin wrapper over #decide (kept for the existing call sites/tests).
    def reject_reason(url, resolve: false)
      decide(url, resolve: resolve).reason
    end

    # Full decision: the rejection reason (or nil) plus the address to pin.
    def decide(url, resolve: false)
      uri = URI.parse(url.to_s)
      unless uri.is_a?(URI::HTTP) && uri.host.present?
        return Decision.new(reason: INVALID_MESSAGE, ipaddr: nil)
      end

      host = uri.hostname # unbrackets IPv6 literals

      # Numeric/literal host: parsed WITHOUT DNS (covers IPv4/IPv6 literals and
      # inet_aton numeric encodings). Kept outside the resolve flag so the
      # request-time syntactic check blocks them too, and so libc never gets to
      # re-parse a numeric string (the Darwin/glibc octal divergence).
      if (ip = host_ip(host))
        return Decision.new(reason: BLOCKED_MESSAGE, ipaddr: nil) if blocked?(ip)

        return Decision.new(reason: nil, ipaddr: ip.to_s)
      end

      # Real hostname: only touch DNS when asked to (delivery time).
      return Decision.new(reason: nil, ipaddr: nil) unless resolve

      addresses = resolve_addresses(host)
      return Decision.new(reason: UNRESOLVABLE_MESSAGE, ipaddr: nil) if addresses.empty?
      if addresses.any? { |addr| (ip = ip_literal(addr)) && blocked?(ip) }
        return Decision.new(reason: BLOCKED_MESSAGE, ipaddr: nil)
      end

      # Pin the first approved address so the connect uses exactly what we vetted.
      Decision.new(reason: nil, ipaddr: addresses.first)
    rescue URI::InvalidURIError
      Decision.new(reason: INVALID_MESSAGE, ipaddr: nil)
    end

    private

    # A host that is itself an address (no DNS): an IP literal, or a numeric
    # IPv4 encoding that libc would accept but IPAddr rejects.
    def host_ip(host)
      ip_literal(host) || inet_aton(host)
    end

    def ip_literal(host)
      IPAddr.new(host)
    rescue IPAddr::InvalidAddressError
      nil
    end

    # Self-implemented inet_aton: normalizes the numeric IPv4 forms libc's
    # getaddrinfo accepts but IPAddr rejects — 1-4 dot-separated parts in
    # decimal, hex (0x...) or octal (0...), the last part absorbing the remaining
    # low bytes (so 2130706433, 0x7f000001, 0177.0.0.1 and 127.1 all normalize
    # to 127.0.0.1). Returns an IPAddr or nil. Doing this ourselves — rather
    # than trusting libc — is deliberate: 0177.0.0.1 is public on Darwin's libc
    # but loopback on glibc, so a resolver-only fix would pass the laptop and
    # stay exploitable in production.
    def inet_aton(host)
      parts = host.split(".", -1)
      return nil if parts.empty? || parts.size > 4

      nums = parts.map { |part| parse_uint(part) }
      return nil if nums.any?(&:nil?)
      return nil if nums[0...-1].any? { |n| n > 255 }
      return nil if nums[-1] >= 256**(5 - nums.size)

      value = nums[0...-1].each_with_index.sum { |n, i| n << (8 * (3 - i)) } + nums[-1]
      IPAddr.new(value, Socket::AF_INET)
    rescue IPAddr::InvalidAddressError, RangeError
      nil
    end

    # Parses one inet_aton part as an unsigned int in decimal, hex (0x...) or
    # octal (0...). Returns nil for anything else (e.g. a DNS label like "com").
    def parse_uint(str)
      case str
      when /\A0[xX][0-9a-fA-F]+\z/ then Integer(str, 16)
      when /\A0[0-7]+\z/           then Integer(str, 8)
      when /\A0\z/                 then 0
      when /\A[1-9][0-9]*\z/       then Integer(str, 10)
      end
    rescue ArgumentError
      nil
    end

    # Resolves via the SAME libc resolver Net::HTTP connects with, so the
    # address we vet is the address the socket would use. Empty on failure.
    def resolve_addresses(host)
      Addrinfo.getaddrinfo(host, nil, nil, :STREAM).map(&:ip_address)
    rescue SocketError
      []
    end

    def blocked?(ip)
      ip = ip.native if ip.ipv6? && ip.ipv4_mapped? # ::ffff:127.0.0.1 → 127.0.0.1
      BLOCKED_RANGES.any? { |range| range.include?(ip) }
    end
  end
end
