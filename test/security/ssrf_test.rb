require "test_helper"

# White-box regression suite from the 2026-07-15 pentest pass
# (docs/security/pentest-2026-07-15.md), focused on CallbackUrlPolicy SSRF
# gaps. test/services/callback_url_policy_test.rb already covers the baseline
# IP-literal / resolve:false / resolve:true blocking behavior; this suite adds
# the specific gaps found during the pentest pass (GED-SEC-08/-09/-10).
class SsrfTest < ActiveSupport::TestCase
  # --- GED-SEC-08: alternate numeric IP encodings (fixed) ---
  #
  # IPAddr.new raises on these numeric loopback encodings, so before the fix
  # #ip_literal treated them as "not an IP" and they slipped past the
  # request-time (resolve: false) 422 guard, then libc's getaddrinfo connected
  # them to 127.0.0.1. CallbackUrlPolicy now normalizes them itself via an
  # inet_aton parser (outside the resolve flag, so no DNS), and blocks them.
  test "decimal-encoded loopback IP is blocked" do
    assert_equal CallbackUrlPolicy::BLOCKED_MESSAGE,
                 CallbackUrlPolicy.reject_reason("http://2130706433/cb")
  end

  test "hex-encoded loopback IP is blocked" do
    assert_equal CallbackUrlPolicy::BLOCKED_MESSAGE,
                 CallbackUrlPolicy.reject_reason("http://0x7f000001/cb")
  end

  test "zero-padded octal-encoded loopback IP is blocked" do
    # Platform trap: 0177.0.0.1 is PUBLIC on Darwin's libc getaddrinfo but
    # LOOPBACK on glibc (the prod image). The policy's own inet_aton normalizes
    # it to 127.0.0.1 on both, so this is blocked identically everywhere.
    assert_equal CallbackUrlPolicy::BLOCKED_MESSAGE,
                 CallbackUrlPolicy.reject_reason("http://0177.0.0.1/cb")
  end

  test "short-form (127.1) loopback IP is blocked" do
    # Fourth encoding, uncovered before the fix: 2-part inet_aton where the
    # final part absorbs the low three bytes -> 127.0.0.1.
    assert_equal CallbackUrlPolicy::BLOCKED_MESSAGE,
                 CallbackUrlPolicy.reject_reason("http://127.1/cb")
  end

  test "a real hostname is not mis-parsed as a numeric IP" do
    # Negative control: "example.com" must fall through to the DNS branch, not
    # be read as an address. With resolve: false that branch is skipped, so the
    # syntactic check returns nil (a genuine numeric loopback returns
    # BLOCKED_MESSAGE here) — proof inet_aton did not claim it.
    assert_nil CallbackUrlPolicy.reject_reason("http://example.com/cb")
  end

  test "an out-of-range dotted-quad is not mis-parsed as a numeric IP" do
    # 999.1.1.1 has a byte > 255, so inet_aton rejects it and it is treated as a
    # hostname (not blocked at resolve: false), rather than wrapping to some
    # in-range address.
    assert_nil CallbackUrlPolicy.reject_reason("http://999.1.1.1/cb")
  end

  # --- GED-SEC-09: DNS-rebinding TOCTOU (fixed by pinning) ---

  test "the callback connects to the pinned address that passed the SSRF check" do
    # Invariant: the address the policy approved is the address the socket
    # connects to. The policy resolves once (Addrinfo, the same libc resolver
    # Net::HTTP uses) and returns it in Decision#ipaddr; the job sets
    # http.ipaddr = that, so libc never re-resolves the hostname at connect time
    # (no rebinding window). The original Host/SNI hostname is preserved.
    approved = "93.184.216.34"
    upload_request = UploadRequest.create!(
      department: Department.create!(name: "Rebind"), filename: "report.pdf",
      callback_url: "http://rebind.test/cb", uploaded_by: "erp-app",
      status: "completed"
    )
    stub_request(:post, "http://rebind.test/cb").to_return(status: 200)

    # WebMock matches on the Net::HTTP @address (the hostname), never on ipaddr,
    # so the pinned connect target is invisible to request matching — capture the
    # instance and assert on it directly.
    pinned = nil
    original = Net::HTTP.method(:new)
    Net::HTTP.define_singleton_method(:new) do |*args, &block|
      pinned = original.call(*args, &block)
    end

    begin
      stub_dns([ approved ]) { DeliverCallbackJob.new.perform(upload_request.id) }
    ensure
      Net::HTTP.singleton_class.remove_method(:new)
      Net::HTTP.define_singleton_method(:new, original)
    end

    assert_equal approved, pinned.ipaddr,
                 "job must connect to the vetted address, not re-resolve the hostname"
    assert_equal "rebind.test", pinned.address,
                 "Host header / SNI / cert validation must keep the original hostname"
  end

  # --- GED-SEC-10: unresolvable host is now refused (was accepted risk) ---

  test "an unresolvable host is refused (nothing to pin, GED-SEC-10 reversed)" do
    # Reversed from the old accepted-risk behavior: under pinning, no resolved
    # address means nothing to pin, so the job cannot connect by construction.
    # The policy refuses it up front with UNRESOLVABLE_MESSAGE rather than
    # letting a hostname fall through to a second, unpinned resolution.
    stub_dns([]) do
      assert_equal CallbackUrlPolicy::UNRESOLVABLE_MESSAGE,
                   CallbackUrlPolicy.reject_reason("http://nowhere.invalid/cb", resolve: true)
    end
  end
end
