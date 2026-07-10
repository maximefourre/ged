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

  test "rejects callback_url values that are not http(s) URLs with 422" do
    [ "notaurl", "file:///etc/passwd", "ftp://caller.test/cb" ].each do |bad_url|
      assert_no_enqueued_jobs do
        post "/api/v1/documents", headers: bearer_header("erp-app"),
             params: { department_id: @finance.id,
                       file: fixture_file_upload("report.pdf", "application/pdf"),
                       callback_url: bad_url }
      end
      assert_response :unprocessable_entity, "expected 422 for callback_url=#{bad_url.inspect}"
      assert_equal 0, UploadRequest.count
    end
  end

  test "upload request status can be polled through pending and completed" do
    stub_request(:post, @callback_url).to_return(status: 200)
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id,
                   file: fixture_file_upload("report.pdf", "application/pdf"),
                   callback_url: @callback_url }
    request_id = JSON.parse(response.body)["request_id"]

    get "/api/v1/upload_requests/#{request_id}", headers: bearer_header("erp-app")
    assert_response :success
    assert_equal "pending", JSON.parse(response.body)["status"]

    perform_enqueued_jobs

    get "/api/v1/upload_requests/#{request_id}", headers: bearer_header("erp-app")
    body = JSON.parse(response.body)
    assert_equal "completed", body["status"]
    assert_equal UploadRequest.find(request_id).document_id, body["document_id"]
    assert_nil body["error_message"]
  end

  test "polling an upload request of a forbidden department returns 403" do
    hr = Department.create!(name: "HR")
    upload_request = UploadRequest.create!(department: hr, filename: "report.pdf",
                                           callback_url: @callback_url,
                                           uploaded_by: "other-app")
    get "/api/v1/upload_requests/#{upload_request.id}", headers: bearer_header("erp-app")
    assert_response :forbidden
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
