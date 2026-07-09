require "net/http"

module Auth
  # Process-local JWKS cache. Refreshes every TTL seconds; keeps serving the
  # last good key set if a refresh fails; fails closed (FetchError) only if
  # no key set has ever been fetched.
  class JwksCache
    class FetchError < StandardError; end

    TTL = 300 # seconds

    class << self
      def instance
        @instance ||= new(ENV.fetch("GED_OIDC_JWKS_URL"))
      end

      def reset!
        @instance = nil
      end
    end

    def initialize(url)
      @url = url
      @mutex = Mutex.new
      @key_set = nil
      @fetched_at = nil
    end

    def key_set
      @mutex.synchronize do
        refresh if @key_set.nil? || stale?
        raise FetchError, "no JWKS available" if @key_set.nil?
        @key_set
      end
    end

    def force_expire_for_tests!
      @fetched_at = Time.at(0)
    end

    private

    def stale?
      @fetched_at.nil? || (Time.now - @fetched_at) > TTL
    end

    def refresh
      response = Net::HTTP.get_response(URI(@url))
      unless response.is_a?(Net::HTTPSuccess)
        raise FetchError, "JWKS endpoint returned #{response.code}"
      end
      @key_set = JWT::JWK::Set.new(JSON.parse(response.body))
      @fetched_at = Time.now
    rescue FetchError, StandardError => e
      Rails.logger.error("JWKS refresh failed: #{e.class}: #{e.message}")
      # keep stale keys if we have them; key_set raises if we never had any
    end
  end
end
