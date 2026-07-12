require "ipaddr"
require "resolv"

# SSRF guard for client-supplied callback URLs: the async upload callback must
# never POST into the internal network. Checked twice with the same rules —
# syntactically at request time (resolve: false, catches IP literals with a
# fast 422) and with DNS resolution at delivery time in DeliverCallbackJob
# (resolve: true, the effective boundary).
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

  class << self
    # Returns nil when the URL is acceptable, or the reason it is not.
    def reject_reason(url, resolve: false)
      uri = URI.parse(url.to_s)
      return INVALID_MESSAGE unless uri.is_a?(URI::HTTP) && uri.host.present?

      host = uri.hostname # unbrackets IPv6 literals
      if (ip = ip_literal(host))
        return BLOCKED_MESSAGE if blocked?(ip)
      elsif resolve && resolves_to_blocked?(host)
        return BLOCKED_MESSAGE
      end
      nil
    rescue URI::InvalidURIError
      INVALID_MESSAGE
    end

    private

    def ip_literal(host)
      IPAddr.new(host)
    rescue IPAddr::InvalidAddressError
      nil
    end

    # An unresolvable host is allowed through: it cannot reach anything, and
    # delivery will fail on its own with the job's normal retry semantics.
    def resolves_to_blocked?(host)
      Resolv.getaddresses(host).any? do |address|
        (ip = ip_literal(address)) && blocked?(ip)
      end
    end

    def blocked?(ip)
      ip = ip.native if ip.ipv6? && ip.ipv4_mapped? # ::ffff:127.0.0.1 → 127.0.0.1
      BLOCKED_RANGES.any? { |range| range.include?(ip) }
    end
  end
end
