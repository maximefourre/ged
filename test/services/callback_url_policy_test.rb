require "test_helper"

class CallbackUrlPolicyTest < ActiveSupport::TestCase
  test "accepts a public hostname without resolving" do
    assert_nil CallbackUrlPolicy.reject_reason("https://example.com/cb")
  end

  test "accepts a public IP literal" do
    assert_nil CallbackUrlPolicy.reject_reason("http://93.184.216.34/cb")
  end

  test "rejects non-http(s) or hostless URLs" do
    [ "ftp://example.com/cb", "not a url", "http://", "javascript:alert(1)" ].each do |url|
      assert_equal "must be an http(s) URL with a host",
                   CallbackUrlPolicy.reject_reason(url), url
    end
  end

  test "rejects blocked IP literals without touching DNS" do
    %w[
      http://127.0.0.1/cb
      http://10.0.0.5/cb
      http://172.16.3.4/cb
      http://192.168.1.1/cb
      http://169.254.169.254/latest/meta-data
      http://100.64.0.1/cb
      http://0.0.0.0/cb
      http://[::1]/cb
      http://[fe80::1]/cb
      http://[fc00::1]/cb
      http://[::ffff:127.0.0.1]/cb
    ].each do |url|
      assert_equal "must not target a private or internal address",
                   CallbackUrlPolicy.reject_reason(url), url
    end
  end

  test "resolve: false never queries DNS" do
    stub_dns do
      assert_nil CallbackUrlPolicy.reject_reason("http://caller.test/cb")
    end
  end

  test "resolve: true rejects a hostname resolving to a blocked address" do
    stub_dns([ "10.0.0.5" ]) do
      assert_equal "must not target a private or internal address",
                   CallbackUrlPolicy.reject_reason("http://evil.test/cb", resolve: true)
    end
  end

  test "resolve: true rejects when any resolved address is blocked" do
    stub_dns([ "93.184.216.34", "127.0.0.1" ]) do
      assert_not_nil CallbackUrlPolicy.reject_reason("http://evil.test/cb", resolve: true)
    end
  end

  test "resolve: true accepts a hostname resolving publicly" do
    stub_dns([ "93.184.216.34" ]) do
      assert_nil CallbackUrlPolicy.reject_reason("http://caller.test/cb", resolve: true)
    end
  end

  test "resolve: true refuses an unresolvable host (nothing to pin)" do
    stub_dns([]) do
      assert_equal CallbackUrlPolicy::UNRESOLVABLE_MESSAGE,
                   CallbackUrlPolicy.reject_reason("http://nowhere.test/cb", resolve: true)
    end
  end
end
