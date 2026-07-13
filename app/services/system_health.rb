# Green/orange/red checks for the admin status page. Every check is local
# (app DB, SolidQueue tables, cached JWKS state, `df`) — never live HTTP on
# page load — and rescues its own errors so the page can't 500 because a
# subsystem is down.
class SystemHealth
  Check = Data.define(:name, :state, :detail) # state: :ok, :warn, :bad, :unknown

  DISK_WARN = 0.80
  DISK_BAD  = 0.90

  class << self
    def checks
      [ database, job_workers, jwks, storage_disk ]
    end

    def database
      ActiveRecord::Base.connection.execute("SELECT 1")
      Check.new(name: "Database", state: :ok, detail: "reachable")
    rescue StandardError => e
      Check.new(name: "Database", state: :bad, detail: e.class.name)
    end

    def job_workers
      threshold = SolidQueue.process_alive_threshold.ago
      workers = SolidQueue::Process.where(kind: "Worker")
      alive = workers.where(last_heartbeat_at: threshold..).count
      if alive.positive?
        Check.new(name: "Job workers", state: :ok, detail: "#{alive} alive")
      elsif workers.exists?
        Check.new(name: "Job workers", state: :bad, detail: "heartbeats stale")
      else
        Check.new(name: "Job workers", state: :bad, detail: "no worker registered")
      end
    rescue StandardError => e
      Check.new(name: "Job workers", state: :bad, detail: e.class.name)
    end

    # Staleness alone is normal (the cache refreshes lazily, on API traffic);
    # only a recorded refresh failure means the IdP is unreachable.
    def jwks
      health = Auth::JwksCache.instance.health
      if !health[:fetched]
        Check.new(name: "OIDC JWKS", state: :unknown, detail: "not fetched yet (no API traffic)")
      elsif health[:failing]
        Check.new(name: "OIDC JWKS", state: :warn,
                  detail: "serving stale keys, last refresh failed (age #{health[:age_seconds].to_i}s)")
      else
        Check.new(name: "OIDC JWKS", state: :ok, detail: "keys cached")
      end
    end

    def storage_disk
      disk = DiskSpace.for(storage_root)
      return Check.new(name: "Storage disk", state: :unknown, detail: "df unavailable") if disk.nil?

      used = disk.used_fraction
      state = used >= DISK_BAD ? :bad : used >= DISK_WARN ? :warn : :ok
      Check.new(name: "Storage disk", state: state, detail: "#{(used * 100).round}% used")
    end

    private

    def storage_root
      ActiveStorage::Blob.service.try(:root) || Rails.root.join("storage")
    end
  end
end
