require "test_helper"

class DeliverCallbackJobTest < ActiveJob::TestCase
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
end
