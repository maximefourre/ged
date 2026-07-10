require "test_helper"

class Admin::AdminUiTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = Admin.create!(username: "alice", password: "secret123")
    sign_in @admin
    @finance = Department.create!(name: "Finance")
  end

  test "creates a top-level department" do
    post "/admin/departments", params: { department: { name: "HR" } }
    assert_redirected_to "/admin/departments"
    assert Department.exists?(name: "HR", parent_id: nil)
  end

  test "creates a subfolder" do
    post "/admin/departments", params: { department: { name: "Invoices", parent_id: @finance.id } }
    assert Department.exists?(name: "Invoices", parent_id: @finance.id)
  end

  test "renames a department" do
    get "/admin/departments/#{@finance.id}/edit"
    assert_response :success

    patch "/admin/departments/#{@finance.id}",
          params: { department: { name: "Finance & Ops" } }
    assert_redirected_to "/admin/departments"
    assert_equal "Finance & Ops", @finance.reload.name
  end

  test "rejects renaming a department to a blank name" do
    patch "/admin/departments/#{@finance.id}", params: { department: { name: "" } }
    assert_response :unprocessable_entity
    assert_equal "Finance", @finance.reload.name
  end

  test "manages a document's tags" do
    doc = Document.new(department: @finance, filename: "report.pdf", uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!
    invoice = Tag.create!(name: "invoice")
    urgent = Tag.create!(name: "urgent")

    get "/admin/documents/#{doc.id}/edit"
    assert_response :success

    patch "/admin/documents/#{doc.id}",
          params: { document: { tag_ids: [ invoice.id, urgent.id ] } }
    assert_redirected_to "/admin/documents"
    assert_equal %w[invoice urgent], doc.reload.tags.order(:name).pluck(:name)

    # unchecking everything (checkbox helper submits only the blank) clears tags
    patch "/admin/documents/#{doc.id}", params: { document: { tag_ids: [ "" ] } }
    assert_equal [], doc.reload.tags.to_a
  end

  test "deletes a department granted to an app client" do
    client = AppClient.create!(client_identifier: "erp-app")
    client.departments << @finance
    delete "/admin/departments/#{@finance.id}"
    assert_redirected_to "/admin/departments"
    assert_not Department.exists?(@finance.id)
  end

  test "deleting a department with upload requests shows an alert instead of a 500" do
    UploadRequest.create!(department: @finance, filename: "report.pdf",
                          callback_url: "https://caller.test/cb",
                          uploaded_by: "erp-app", status: "pending")
    delete "/admin/departments/#{@finance.id}"
    assert_redirected_to "/admin/departments"
    assert flash[:alert].present?
    assert Department.exists?(@finance.id)
  end

  test "creates and deletes a tag" do
    post "/admin/tags", params: { tag: { name: "Invoice" } }
    tag = Tag.find_by(name: "invoice")
    assert tag
    delete "/admin/tags/#{tag.id}"
    assert_not Tag.exists?(tag.id)
  end

  test "registers an app client with department access" do
    post "/admin/app_clients", params: {
      app_client: { client_identifier: "erp-app", description: "ERP",
                    department_ids: [ @finance.id ] }
    }
    client = AppClient.find_by(client_identifier: "erp-app")
    assert_equal [ @finance.id ], client.department_ids
  end

  test "uploads and deletes a document" do
    post "/admin/documents", params: {
      document: { department_id: @finance.id,
                  file: fixture_file_upload("report.pdf", "application/pdf") }
    }
    doc = Document.find_by(filename: "report.pdf")
    assert doc
    assert_equal "admin:alice", doc.uploaded_by

    get "/admin/documents"
    assert_response :success
    assert_includes response.body, "report.pdf"

    delete "/admin/documents/#{doc.id}"
    assert_not Document.exists?(doc.id)
  end

  test "uploading a non-file file param shows an alert instead of a 500" do
    post "/admin/documents", params: {
      document: { department_id: @finance.id, file: "hello" }
    }
    assert_redirected_to "/admin/documents/new"
    assert flash[:alert].present?
    assert_equal 0, Document.count
  end

  test "lists upload requests with status and error" do
    UploadRequest.create!(department: @finance, filename: "big.pdf",
                          callback_url: "https://caller.test/cb",
                          uploaded_by: "erp-app", status: "failed",
                          error_message: "staged file lost before processing")
    get "/admin/upload_requests"
    assert_response :success
    assert_includes response.body, "big.pdf"
    assert_includes response.body, "failed"
    assert_includes response.body, "staged file lost before processing"
  end

  test "downloads a document" do
    doc = Document.new(department: @finance, filename: "report.pdf", uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!
    get "/admin/documents/#{doc.id}/download"
    assert_equal "PDF!", response.body
  end
end
