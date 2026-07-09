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
end
