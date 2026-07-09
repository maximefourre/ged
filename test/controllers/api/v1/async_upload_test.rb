require "test_helper"

class Api::V1::AsyncUploadTest < ActionDispatch::IntegrationTest
  include ActiveJob::TestHelper

  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance
    @callback_url = "https://caller.test/ged-callback"
  end

  test "upload with callback_url returns 202 and processes in background" do
    stub_request(:post, @callback_url).to_return(status: 200)

    perform_enqueued_jobs do
      post "/api/v1/documents", headers: bearer_header("erp-app"),
           params: { department_id: @finance.id,
                     file: fixture_file_upload("report.pdf", "application/pdf"),
                     callback_url: @callback_url }
      assert_response :accepted
    end

    body = JSON.parse(response.body)
    request_id = body["request_id"]
    assert_equal "pending", body["status"]

    upload_request = UploadRequest.find(request_id)
    assert_equal "completed", upload_request.status
    document = Document.find(upload_request.document_id)
    assert document.file.attached?
    assert_equal "PDF!", document.file.download
    assert_not File.exist?(StagingStore.path(request_id))

    assert_requested(:post, @callback_url) do |req|
      payload = JSON.parse(req.body)
      payload["request_id"] == request_id &&
        payload["status"] == "completed" &&
        payload["document_id"] == upload_request.document_id
    end
  end

  test "async upload into a forbidden department returns 403 and enqueues nothing" do
    hr = Department.create!(name: "HR")
    assert_no_enqueued_jobs do
      post "/api/v1/documents", headers: bearer_header("erp-app"),
           params: { department_id: hr.id,
                     file: fixture_file_upload("report.pdf", "application/pdf"),
                     callback_url: @callback_url }
    end
    assert_response :forbidden
  end
end
