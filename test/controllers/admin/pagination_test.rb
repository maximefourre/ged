require "test_helper"

class Admin::PaginationTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    sign_in Admin.create!(username: "alice", password: "secret123")
    @finance = Department.create!(name: "Finance")
  end

  test "documents index shows 50 per page with navigation links" do
    51.times { |i| Document.create!(department: @finance, filename: "doc-#{i}.pdf", uploaded_by: "seed") }

    get admin_documents_path
    assert_response :success
    assert_select "table tr", 51 # header + 50 rows
    assert_select "a", text: "Older →"
    assert_select "a", { text: "← Newer", count: 0 }

    get admin_documents_path(page: 2)
    assert_response :success
    assert_select "table tr", 2 # header + 1 row
    assert_select "a", text: "← Newer"
    assert_select "a", { text: "Older →", count: 0 }
  end

  test "upload requests index shows 50 per page and keeps the status filter" do
    51.times do
      UploadRequest.create!(department: @finance, filename: "report.pdf",
                            callback_url: "https://caller.test/cb",
                            uploaded_by: "erp-app", status: "failed")
    end

    get admin_upload_requests_path(status: "failed")
    assert_response :success
    assert_select "table tr", 51
    assert_select "a[href*='status=failed'][href*='page=2']", text: "Older →"
  end

  test "non-numeric or non-scalar page params fall back to the first page" do
    Document.create!(department: @finance, filename: "doc.pdf", uploaded_by: "seed")
    [ "abc", "-1", [ "2" ] ].each do |page|
      get admin_documents_path(page: page)
      assert_response :success, "page=#{page.inspect}"
      assert_select "table tr", 2
    end
  end
end
