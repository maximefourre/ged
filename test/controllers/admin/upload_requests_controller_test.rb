require "test_helper"

class Admin::UploadRequestsControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = Admin.create!(username: "alice", password: "secret123")
    @finance = Department.create!(name: "Finance")
  end

  def create_request(filename:, status: "pending")
    UploadRequest.create!(department: @finance, filename: filename,
                          callback_url: "https://caller.test/cb",
                          uploaded_by: "erp-app", status: status)
  end

  test "requires an admin session" do
    get "/admin/upload_requests"
    assert_redirected_to "/admins/sign_in"
  end

  test "index lists requests with their department name" do
    sign_in @admin
    create_request(filename: "report.pdf")

    get "/admin/upload_requests"
    assert_response :success
    assert_includes response.body, "report.pdf"
    assert_includes response.body, "Finance"
  end

  test "index renders for a request whose department is trashed" do
    sign_in @admin
    create_request(filename: "report.pdf")
    @finance.soft_delete!

    # Must not 500 when the department is soft-deleted (default_scope hides it).
    get "/admin/upload_requests"
    assert_response :success
    assert_includes response.body, "report.pdf"
    assert_includes response.body, "Finance"
    assert_includes response.body, "(trashed)"
  end

  test "index shows an empty state when there are no upload requests" do
    sign_in @admin
    get admin_upload_requests_path
    assert_response :success
    assert_match "No upload requests match.", response.body
  end
end
