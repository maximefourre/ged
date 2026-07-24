require "test_helper"

class Admin::DocumentsControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = Admin.create!(username: "alice", password: "secret123")
    @path = Rails.root.join("test/fixtures/files/report.pdf")
    @document = Document.new(
      department: Department.create!(name: "Finance"),
      filename: "report.pdf",
      content_type: "application/pdf",
      byte_size: File.size(@path),
      uploaded_by: "admin:admin"
    )
    @document.file.attach(io: File.open(@path), filename: "report.pdf",
                          content_type: "application/pdf")
    @document.save!
  end

  test "preview streams the file inline with hardening headers, signed in" do
    sign_in @admin

    get preview_admin_document_path(@document)

    assert_response :success
    assert_match(/^inline/, response.headers["Content-Disposition"])
    assert_equal "nosniff", response.headers["X-Content-Type-Options"]
    csp = response.headers["Content-Security-Policy"]
    assert csp.present?
    assert_includes csp, "sandbox"
    assert_includes csp, "default-src 'none'"
    assert_equal File.binread(@path), response.body
  end

  test "preview redirects to login when signed out" do
    get preview_admin_document_path(@document)

    assert_response :redirect
    assert_redirected_to new_admin_session_path
  end

  test "download still streams as an attachment" do
    sign_in @admin

    get download_admin_document_path(@document)

    assert_match(/^attachment/, response.headers["Content-Disposition"])
  end
end
