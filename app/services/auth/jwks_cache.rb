require "net/http"

module Auth
  # Process-local JWKS cache. Refreshes every TTL seconds; keeps serving the
  # last good key set if a refresh fails; fails closed (FetchError) only if
  # no key set has ever been fetched.
  #
  # Refreshes are single-flight: while one thread fetches, concurrent requests
  # keep being served the cached (possibly stale) keys instead of queueing on
  # the lock. Fetches use short timeouts, back off after a failure, and a
  # forced refresh (unknown kid — IdP key rotation) is rate-limited so garbage
  # tokens can't hammer the IdP.
  class JwksCache
    class FetchError < StandardError; end

    TTL = 300 # seconds
    FAILURE_BACKOFF = 30 # seconds without retrying after a failed fetch
    FORCED_REFRESH_INTERVAL = 30 # min seconds between unknown-kid refreshes
    OPEN_TIMEOUT = 2 # seconds
    READ_TIMEOUT = 3 # seconds

    INSTANCE_MUTEX = Mutex.new

    class << self
      # double-checked locking: without it, two threads hitting a fresh
      # process could each build (and fetch through) their own instance
      def instance
        @instance || INSTANCE_MUTEX.synchronize do
          @instance ||= new(ENV.fetch("GED_OIDC_JWKS_URL"))
        end
      end

      def reset!
        INSTANCE_MUTEX.synchronize { @instance = nil }
      end
    end

    def initialize(url)
      @url = url
      @mutex = Mutex.new
      @key_set = nil
      @fetched_at = nil
      @failed_at = nil
      @forced_at = nil
    end

    def key_set(force: false)
      refresh_single_flight(force) if wants_refresh?(force)
      @key_set || raise(FetchError, "no JWKS available")
    end

    def force_expire_for_tests!
      @fetched_at = now - TTL - 1
    end

    private

    def wants_refresh?(force)
      return false if @failed_at && now - @failed_at < FAILURE_BACKOFF
      if force
        @forced_at.nil? || now - @forced_at >= FORCED_REFRESH_INTERVAL
      else
        @key_set.nil? || @fetched_at.nil? || now - @fetched_at > TTL
      end
    end

    # When no keys were ever fetched there is nothing to serve, so requests
    # block on the mutex until the in-flight fetch completes. Once a key set
    # exists, whoever wins try_lock refreshes and everyone else is served the
    # cached keys immediately.
    def refresh_single_flight(force)
      if @key_set.nil?
        @mutex.synchronize { refresh(force) if wants_refresh?(force) }
      elsif @mutex.try_lock
        begin
          refresh(force) if wants_refresh?(force)
        ensure
          @mutex.unlock
        end
      end
    end

    def refresh(force)
      @forced_at = now if force
      uri = URI(@url)
      response = Net::HTTP.start(uri.host, uri.port, use_ssl: uri.scheme == "https",
                                 open_timeout: OPEN_TIMEOUT, read_timeout: READ_TIMEOUT) do |http|
        http.get(uri.request_uri)
      end
      unless response.is_a?(Net::HTTPSuccess)
        raise FetchError, "JWKS endpoint returned #{response.code}"
      end
      @key_set = JWT::JWK::Set.new(JSON.parse(response.body))
      @fetched_at = now
      @failed_at = nil
    rescue StandardError => e
      @failed_at = now
      Rails.logger.error("JWKS refresh failed: #{e.class}: #{e.message}")
      # keep stale keys if we have them; key_set raises if we never had any
    end

    def now
      Process.clock_gettime(Process::CLOCK_MONOTONIC)
    end
  end
end
