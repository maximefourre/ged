require "test_helper"

class SystemHealthTest < ActiveSupport::TestCase
  test "database check is ok when the database answers" do
    check = SystemHealth.database
    assert_equal :ok, check.state
  end

  test "job workers check is ok when a worker heartbeat is fresh" do
    create_worker!(last_heartbeat_at: Time.current)
    check = SystemHealth.job_workers
    assert_equal :ok, check.state
    assert_match "1 alive", check.detail
  end

  test "job workers check is bad when all heartbeats are stale" do
    create_worker!(last_heartbeat_at: (SolidQueue.process_alive_threshold + 1.minute).ago)
    check = SystemHealth.job_workers
    assert_equal :bad, check.state
    assert_match "stale", check.detail
  end

  test "job workers check is bad when no worker is registered" do
    check = SystemHealth.job_workers
    assert_equal :bad, check.state
    assert_match "no worker", check.detail
  end

  test "jwks check is unknown before any fetch and does no HTTP" do
    check = SystemHealth.jwks
    assert_equal :unknown, check.state
    assert_not_requested(:get, ENV.fetch("GED_OIDC_JWKS_URL"))
  end

  test "jwks check is ok after a successful fetch" do
    stub_jwks!
    Auth::JwksCache.instance.key_set
    assert_equal :ok, SystemHealth.jwks.state
  end

  test "jwks check warns when serving stale keys after a failed refresh" do
    stub_jwks!
    cache = Auth::JwksCache.instance
    cache.key_set
    cache.force_expire_for_tests!
    stub_request(:get, ENV.fetch("GED_OIDC_JWKS_URL")).to_return(status: 500)
    cache.key_set # refresh fails; stale keys keep being served
    assert_equal :warn, SystemHealth.jwks.state
  end

  test "storage disk check maps used fraction to thresholds" do
    { 0.50 => :ok, 0.85 => :warn, 0.95 => :bad }.each do |fraction, expected|
      result = DiskSpace::Result.new(total_kb: 100, available_kb: ((1 - fraction) * 100).round)
      stub_disk_space(result) do
        assert_equal expected, SystemHealth.storage_disk.state, "#{fraction} used should be #{expected}"
      end
    end
  end

  test "storage disk check is unknown when df is unavailable" do
    stub_disk_space(nil) do
      assert_equal :unknown, SystemHealth.storage_disk.state
    end
  end

  test "checks returns database, workers, jwks and disk in display order" do
    checks = SystemHealth.checks
    assert_equal [ "Database", "Job workers", "OIDC JWKS", "Storage disk" ], checks.map(&:name)
  end

  private

  # minitest/mock isn't bundled with minitest 6 — hand-rolled singleton swap,
  # same pattern as DnsTestHelpers. Safe: parallel workers are forked.
  def stub_disk_space(result)
    original = DiskSpace.method(:for)
    DiskSpace.define_singleton_method(:for) { |_path| result }
    yield
  ensure
    DiskSpace.singleton_class.remove_method(:for)
    DiskSpace.define_singleton_method(:for, original)
  end

  def create_worker!(last_heartbeat_at:)
    SolidQueue::Process.create!(
      kind: "Worker", last_heartbeat_at: last_heartbeat_at,
      pid: 12_345, name: "worker-test-#{SecureRandom.hex(4)}"
    )
  end
end
