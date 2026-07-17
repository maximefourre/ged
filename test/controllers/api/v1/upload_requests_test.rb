require "test_helper"

class Api::V1::UploadRequestsTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance
  end

  test "GET /api/v1/upload_requests/:id for trashed dept returns 404" do
    upload_request = UploadRequest.create!(
      department_id: @finance.id,
      callback_url: "https://example.com/callback",
      filename: "report.pdf",
      uploaded_by: "erp-app"
    )

    # Soft-delete the department
    @finance.soft_delete!

    get "/api/v1/upload_requests/#{upload_request.id}",
        headers: bearer_header("erp-app")

    assert_response :not_found
  end

  test "GET /api/v1/upload_requests/:id for live dept returns 200" do
    upload_request = UploadRequest.create!(
      department_id: @finance.id,
      callback_url: "https://example.com/callback",
      filename: "report.pdf",
      uploaded_by: "erp-app"
    )

    get "/api/v1/upload_requests/#{upload_request.id}",
        headers: bearer_header("erp-app")

    assert_response :success
    body = JSON.parse(response.body)
    assert_equal upload_request.id.to_s, body["request_id"]
  end
end
