require "test_helper"

class Admin::StatusControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = Admin.create!(username: "alice", password: "secret123")
    sign_in @admin
    @finance = Department.create!(name: "Finance")
  end

  test "requires an admin session" do
    sign_out @admin
    get "/admin/status"
    assert_redirected_to "/admins/sign_in"
  end

  test "renders the three sections with live counts" do
    UploadRequest.create!(department: @finance, filename: "r.pdf",
                          callback_url: "https://caller.test/cb",
                          uploaded_by: "erp-app", status: "pending")
    DocumentCreator.call(department_id: @finance.id, io: StringIO.new("pdf bytes"),
                         filename: "a.pdf", content_type: "application/pdf",
                         byte_size: 9, uploaded_by: "erp-app")

    get "/admin/status"
    assert_response :success
    assert_select "h1", "Status"
    # system health renders all four checks
    assert_includes response.body, "Database"
    assert_includes response.body, "Job workers"
    assert_includes response.body, "OIDC JWKS"
    assert_includes response.body, "Storage disk"
    # jwks was never fetched in this test — and no WebMock stub exists, which
    # proves the page did no HTTP
    assert_includes response.body, "not fetched yet"
    # pipeline + documents counts
    assert_includes response.body, "pending"
    assert_includes response.body, "Finance"
  end

  test "reflects the uploads queue pause state" do
    get "/admin/status"
    assert_includes response.body, "active"

    UploadQueue.pause!
    get "/admin/status"
    assert_includes response.body, "paused"
  end

  test "top departments table lists stored volume and document count" do
    2.times do |i|
      DocumentCreator.call(department_id: @finance.id, io: StringIO.new("pdf bytes"),
                           filename: "doc-#{i}.pdf", content_type: "application/pdf",
                           byte_size: 9, uploaded_by: "erp-app")
    end

    get "/admin/status"
    assert_response :success
    assert_select "th", "Documents"
    finance_row = css_select("table tbody tr").find { |tr| tr.text.include?("Finance") }
    assert_not_nil finance_row, "expected a top-departments row for Finance"
    assert_equal "2", finance_row.css("td").last.text.strip,
                 "document count should be the last column"
  end

  test "lists recent failed jobs" do
    job = SolidQueue::Job.create!(queue_name: "default", class_name: "DeliverCallbackJob")
    SolidQueue::FailedExecution.create!(job: job,
                                        error: { "exception_class" => "SocketError",
                                                 "message" => "boom" })

    get "/admin/status"
    assert_response :success
    assert_includes response.body, "DeliverCallbackJob"
    assert_includes response.body, "SocketError"
  end
end
