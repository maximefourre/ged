require "test_helper"

class DeliverCallbackJobTest < ActiveJob::TestCase
  include TelemetryTestHelpers

  setup do
    finance = Department.create!(name: "Finance")
    @request = UploadRequest.create!(department: finance, filename: "report.pdf",
                                     callback_url: "https://caller.test/cb",
                                     uploaded_by: "erp-app", status: "completed")
  end

  test "posts the payload to the callback url" do
    stub_request(:post, "https://caller.test/cb").to_return(status: 200)
    DeliverCallbackJob.new.perform(@request.id)
    assert_requested(:post, "https://caller.test/cb") do |req|
      JSON.parse(req.body)["request_id"] == @request.id
    end
  end

  test "includes the error message for failed requests" do
    @request.update!(status: "failed",
                     error_message: "staged file lost before processing")
    stub_request(:post, "https://caller.test/cb").to_return(status: 200)
    DeliverCallbackJob.new.perform(@request.id)
    assert_requested(:post, "https://caller.test/cb") do |req|
      body = JSON.parse(req.body)
      body["status"] == "failed" &&
        body["error_message"] == "staged file lost before processing"
    end
  end

  test "raises DeliveryFailed on non-2xx so retry_on kicks in" do
    stub_request(:post, "https://caller.test/cb").to_return(status: 500)
    assert_raises(DeliverCallbackJob::DeliveryFailed) do
      DeliverCallbackJob.new.perform(@request.id)
    end
  end

  test "raises DeliveryFailed on connection errors" do
    stub_request(:post, "https://caller.test/cb").to_raise(Errno::ECONNREFUSED)
    assert_raises(DeliverCallbackJob::DeliveryFailed) do
      DeliverCallbackJob.new.perform(@request.id)
    end
  end

  test "raises DeliveryFailed on connection reset" do
    stub_request(:post, "https://caller.test/cb").to_raise(Errno::ECONNRESET)
    assert_raises(DeliverCallbackJob::DeliveryFailed) do
      DeliverCallbackJob.new.perform(@request.id)
    end
  end

  test "raises DeliveryFailed on SSL errors" do
    stub_request(:post, "https://caller.test/cb").to_raise(OpenSSL::SSL::SSLError)
    assert_raises(DeliverCallbackJob::DeliveryFailed) do
      DeliverCallbackJob.new.perform(@request.id)
    end
  end

  test "raises DeliveryFailed on unreachable host or network" do
    stub_request(:post, "https://caller.test/cb").to_raise(Errno::EHOSTUNREACH)
    assert_raises(DeliverCallbackJob::DeliveryFailed) do
      DeliverCallbackJob.new.perform(@request.id)
    end
  end

  test "raises DeliveryFailed on timeout" do
    stub_request(:post, "https://caller.test/cb").to_timeout
    assert_raises(DeliverCallbackJob::DeliveryFailed) do
      DeliverCallbackJob.new.perform(@request.id)
    end
  end

  test "refuses a callback host resolving to a private address, without retrying" do
    stub_request(:post, "https://caller.test/cb")
    stub_dns([ "10.0.0.5" ]) do
      assert_nothing_raised { DeliverCallbackJob.new.perform(@request.id) }
    end
    assert_not_requested(:post, "https://caller.test/cb")
  end

  test "refuses an IP-literal loopback callback slipped past validation" do
    # update_column bypasses validation, like a row created before the policy
    @request.update_column(:callback_url, "http://127.0.0.1/cb")
    stub_request(:post, "http://127.0.0.1/cb")
    assert_nothing_raised { DeliverCallbackJob.new.perform(@request.id) }
    assert_not_requested(:post, "http://127.0.0.1/cb")
  end

  test "delivers normally when the host resolves publicly" do
    stub_request(:post, "https://caller.test/cb").to_return(status: 200)
    stub_dns([ "93.184.216.34" ]) do
      DeliverCallbackJob.new.perform(@request.id)
    end
    assert_requested(:post, "https://caller.test/cb")
  end

  test "emits outcome=success on delivery" do
    stub_request(:post, "https://caller.test/cb").to_return(status: 200)
    calls = capture_telemetry(:callback_deliveries) do
      DeliverCallbackJob.new.perform(@request.id)
    end
    assert_equal [ [ 1, { "outcome" => "success" } ] ], calls
  end

  test "emits outcome=failure on a failed attempt" do
    stub_request(:post, "https://caller.test/cb").to_return(status: 500)
    calls = capture_telemetry(:callback_deliveries) do
      assert_raises(DeliverCallbackJob::DeliveryFailed) do
        DeliverCallbackJob.new.perform(@request.id)
      end
    end
    assert_equal [ [ 1, { "outcome" => "failure" } ] ], calls
  end

  test "emits outcome=failure on a connection error" do
    stub_request(:post, "https://caller.test/cb").to_raise(Errno::ECONNREFUSED)
    calls = capture_telemetry(:callback_deliveries) do
      assert_raises(DeliverCallbackJob::DeliveryFailed) do
        DeliverCallbackJob.new.perform(@request.id)
      end
    end
    assert_equal [ [ 1, { "outcome" => "failure" } ] ], calls
  end

  test "emits outcome=refused for a blocked callback target" do
    stub_request(:post, "https://caller.test/cb")
    calls = capture_telemetry(:callback_deliveries) do
      stub_dns([ "10.0.0.5" ]) { DeliverCallbackJob.new.perform(@request.id) }
    end
    assert_equal [ [ 1, { "outcome" => "refused" } ] ], calls
  end

  test "emits outcome=exhausted once retries run out" do
    stub_request(:post, "https://caller.test/cb").to_return(status: 500)
    calls = capture_telemetry(:callback_deliveries) do
      perform_enqueued_jobs do
        DeliverCallbackJob.perform_later(@request.id)
      end
    end
    assert_equal 5, calls.count { |_value, attrs| attrs["outcome"] == "failure" }
    assert_equal 1, calls.count { |_value, attrs| attrs["outcome"] == "exhausted" }
  end
end
