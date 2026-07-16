require "test_helper"

# Branch sec/pagination-and-admin-param-guards: the admin index actions have no
# rescue_from, so a crafted non-scalar query param that reaches finder/model
# code renders a raw Rails 500 (GED-SEC-06). Admin::BaseController#scalar_param
# now silently ignores non-String params, so these must all render 200.
class Admin::ParamGuardsTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    sign_in Admin.create!(username: "alice", password: "secret123")
    @department = Department.create!(name: "Finance")
    Document.create!(department: @department, filename: "doc.pdf", uploaded_by: "seed")
  end

  test "documents index ignores an array filename param instead of 500ing" do
    get admin_documents_path(filename: [ "x" ])
    assert_response :success
  end

  test "documents index ignores a hash department_id param instead of 500ing" do
    get admin_documents_path(department_id: { a: "b" })
    assert_response :success
  end

  test "documents index ignores an array tag param instead of 500ing" do
    get admin_documents_path(tag: [ "x" ])
    assert_response :success
  end

  test "upload requests index ignores a hash status param instead of 500ing" do
    UploadRequest.create!(department: @department, filename: "report.pdf",
                          callback_url: "https://caller.test/cb",
                          uploaded_by: "erp-app", status: "failed")
    get admin_upload_requests_path(status: { a: "b" })
    assert_response :success
  end
end
