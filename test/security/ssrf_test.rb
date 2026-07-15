require "test_helper"

# White-box regression suite from the 2026-07-15 pentest pass
# (docs/security/pentest-2026-07-15.md), focused on CallbackUrlPolicy SSRF
# gaps. test/services/callback_url_policy_test.rb already covers the
# baseline IP-literal / resolve:false / resolve:true blocking behavior; this
# suite adds the specific gaps found during the pentest pass.
class SsrfTest < ActiveSupport::TestCase
  # --- GED-SEC-08: alternate IP encodings ---
  #
  # OBSERVED (ran with skip removed first): IPAddr.new raises
  # IPAddr::InvalidAddressError on all three of these strings (decimal,
  # hex, and zero-padded-octal encoded loopback), so
  # CallbackUrlPolicy#ip_literal treats them as "not an IP literal" and they
  # fall through to the DNS-resolution branch. With resolve: false (the
  # syntactic check run at UploadRequest creation) they are NOT blocked at
  # all (reject_reason returns nil) — a client can pass
  # "http://2130706433/cb" (decimal for 127.0.0.1) straight through the
  # request-time 422 guard. IPAddr does NOT already block these.
  test "decimal-encoded loopback IP is blocked" do
    skip "documented finding GED-SEC-08 — see docs/security/pentest-2026-07-15.md"

    assert_equal CallbackUrlPolicy::BLOCKED_MESSAGE,
                 CallbackUrlPolicy.reject_reason("http://2130706433/cb")
  end

  test "hex-encoded loopback IP is blocked" do
    skip "documented finding GED-SEC-08 — see docs/security/pentest-2026-07-15.md"

    assert_equal CallbackUrlPolicy::BLOCKED_MESSAGE,
                 CallbackUrlPolicy.reject_reason("http://0x7f000001/cb")
  end

  test "zero-padded octal-encoded loopback IP is blocked" do
    skip "documented finding GED-SEC-08 — see docs/security/pentest-2026-07-15.md"

    assert_equal CallbackUrlPolicy::BLOCKED_MESSAGE,
                 CallbackUrlPolicy.reject_reason("http://0177.0.0.1/cb")
  end

  # --- GED-SEC-09: DNS-rebinding TOCTOU ---

  test "the callback connects to the pinned address that passed the SSRF check" do
    skip "documented finding GED-SEC-09 — see docs/security/pentest-2026-07-15.md"

    # Desired invariant: the address the policy approved must be the address
    # the job actually connects to. Today reject_reason resolves the hostname
    # via Resolv to decide "is this safe?", then Net::HTTP.start re-resolves
    # that same hostname independently through libc at connect time — two
    # lookups with a rebinding window between them, so an attacker who answers
    # the first with a public IP and the second with a private one slips
    # through. The fix is to resolve once and connect to that pinned address
    # (carrying the original Host header). Asserting on the connect target is
    # what makes this a real gap test: today the job is handed the *hostname*,
    # which can re-resolve to anything.
    approved = "93.184.216.34"
    upload_request = UploadRequest.create!(
      department: Department.create!(name: "Rebind"), filename: "report.pdf",
      callback_url: "http://rebind.test/cb", uploaded_by: "erp-app",
      status: "completed"
    )
    # Stub both shapes so the test turns green on the fix (connect to the
    # pinned IP) rather than failing on an unstubbed request.
    stub_request(:post, "http://rebind.test/cb").to_return(status: 200)
    stub_request(:post, "http://#{approved}/cb").to_return(status: 200)

    connected_to = nil
    original = Net::HTTP.method(:start)
    Net::HTTP.define_singleton_method(:start) do |host, *rest, &block|
      connected_to = host
      original.call(host, *rest, &block)
    end

    begin
      stub_dns([ approved ]) { DeliverCallbackJob.new.perform(upload_request.id) }
    ensure
      Net::HTTP.singleton_class.remove_method(:start)
      Net::HTTP.define_singleton_method(:start, original)
    end

    assert_equal approved, connected_to,
                 "job connected to #{connected_to.inspect}, which re-resolves " \
                 "independently of the address the SSRF check approved"
  end

  # --- GED-SEC-10: unresolvable host is allowed through, by design ---

  test "an unresolvable host is accepted (documented accepted risk, GED-SEC-10)" do
    # Documenting CURRENT (intended) behavior as a passing test, not a gap:
    # CallbackUrlPolicy deliberately lets an unresolvable hostname through
    # when resolve: true — see the comment above
    # CallbackUrlPolicy.resolves_to_blocked? ("delivery will fail on its own
    # with the job's normal retry semantics"). GED-SEC-10 names this as an
    # accepted risk rather than a bug: it cannot reach anything internal,
    # since it resolves to nothing.
    stub_dns([]) do
      assert_nil CallbackUrlPolicy.reject_reason("http://nowhere.invalid/cb", resolve: true)
    end
  end
end
