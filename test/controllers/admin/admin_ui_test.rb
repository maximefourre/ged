require "test_helper"
require "zip"

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

  test "manages a document's tags through the key=value textarea" do
    doc = Document.new(department: @finance, filename: "report.pdf", uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!

    get "/admin/documents/#{doc.id}/edit"
    assert_response :success

    patch "/admin/documents/#{doc.id}",
          params: { document: { tags_text: "Type=Facture\nurgent" } }
    assert_redirected_to "/admin/documents"
    assert_equal({ "type" => "Facture", "urgent" => nil }, doc.reload.tags)

    # an empty textarea clears the map
    patch "/admin/documents/#{doc.id}", params: { document: { tags_text: "" } }
    assert_equal({}, doc.reload.tags)

    # an invalid map redirects back to the form with an alert, tags untouched
    patch "/admin/documents/#{doc.id}",
          params: { document: { tags_text: "#{"k" * 101}=x" } }
    assert_redirected_to "/admin/documents/#{doc.id}/edit"
    assert_equal({}, doc.reload.tags)
  end

  test "the tags edit page shows readonly document metadata" do
    doc = Document.new(department: @finance, filename: "report.pdf", uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!

    get "/admin/documents/#{doc.id}/edit"
    assert_response :success
    assert_includes response.body, "Upload mode"
    assert_includes response.body, "sync"
    assert_includes response.body, "Indexed"
  end

  test "filters documents by tag key" do
    doc = Document.new(department: @finance, filename: "tagged.pdf", uploaded_by: "seed",
                       tags: { "type" => "facture" })
    doc.file.attach(io: StringIO.new("PDF!"), filename: "tagged.pdf",
                    content_type: "application/pdf")
    doc.save!

    get "/admin/documents", params: { tag: "type" }
    assert_includes response.body, "tagged.pdf"
    # Tags now render as key/value chips rather than "type=facture" text.
    assert_includes response.body, "facture"

    get "/admin/documents", params: { tag: "absent" }
    assert_not_includes response.body, "tagged.pdf"
  end

  test "deletes a department granted to an app client" do
    client = AppClient.create!(client_identifier: "erp-app")
    client.departments << @finance
    delete "/admin/departments/#{@finance.id}"
    assert_redirected_to "/admin/departments"
    assert_not Department.exists?(@finance.id)
  end

  test "deleting a department with upload requests redirects to the resolution page (no 500)" do
    UploadRequest.create!(department: @finance, filename: "report.pdf",
                          callback_url: "https://caller.test/cb",
                          uploaded_by: "erp-app", status: "pending")
    delete "/admin/departments/#{@finance.id}"
    assert_redirected_to confirm_deletion_admin_department_path(@finance)
    assert Department.exists?(@finance.id)
  end

  test "deleting a department whose only documents are soft-deleted redirects to the resolution page (no 500)" do
    doc = Document.new(department: @finance, filename: "trashed.pdf", uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "trashed.pdf",
                    content_type: "application/pdf")
    doc.save!
    doc.soft_delete!

    delete "/admin/departments/#{@finance.id}"
    assert_redirected_to confirm_deletion_admin_department_path(@finance)
    assert Department.exists?(@finance.id)
    assert Document.with_deleted.exists?(doc.id)
  end

  test "registers an app client with full read/write access from the grid" do
    post "/admin/app_clients", params: {
      app_client: { client_identifier: "erp-app", description: "ERP",
                    access: { @finance.id.to_s => { read: "1", write: "1" } } }
    }
    client = AppClient.find_by(client_identifier: "erp-app")
    access = client.client_department_accesses.find_by!(department: @finance)
    assert access.can_read?
    assert access.can_write?
  end

  test "registers a read-only app client" do
    post "/admin/app_clients", params: {
      app_client: { client_identifier: "reader-app",
                    access: { @finance.id.to_s => { read: "1", write: "0" } } }
    }
    access = AppClient.find_by(client_identifier: "reader-app")
                      .client_department_accesses.find_by!(department: @finance)
    assert access.can_read?
    assert_not access.can_write?
  end

  test "registers a write-only app client" do
    post "/admin/app_clients", params: {
      app_client: { client_identifier: "writer-app",
                    access: { @finance.id.to_s => { read: "0", write: "1" } } }
    }
    access = AppClient.find_by(client_identifier: "writer-app")
                      .client_department_accesses.find_by!(department: @finance)
    assert_not access.can_read?
    assert access.can_write?
  end

  test "unchecking both boxes removes the department access row" do
    client = AppClient.create!(client_identifier: "erp-app")
    client.client_department_accesses.create!(department: @finance)

    patch "/admin/app_clients/#{client.id}", params: {
      app_client: { client_identifier: "erp-app",
                    access: { @finance.id.to_s => { read: "0", write: "0" } } }
    }
    assert_redirected_to "/admin/app_clients"
    assert_not client.client_department_accesses.exists?(department: @finance)
  end

  test "updating access flags edits the existing row in place" do
    client = AppClient.create!(client_identifier: "erp-app")
    client.client_department_accesses.create!(department: @finance)

    patch "/admin/app_clients/#{client.id}", params: {
      app_client: { client_identifier: "erp-app",
                    access: { @finance.id.to_s => { read: "1", write: "0" } } }
    }
    access = client.client_department_accesses.find_by!(department: @finance)
    assert access.can_read?
    assert_not access.can_write?
  end

  test "the app client form shows the read/write grid" do
    get "/admin/app_clients/new"
    assert_response :success
    assert_includes response.body, "Department access"
    assert_includes response.body, "app_client[access][#{@finance.id}][read]"
    assert_includes response.body, "app_client[access][#{@finance.id}][write]"
  end

  test "the legacy department_ids param no longer grants access" do
    post "/admin/app_clients", params: {
      app_client: { client_identifier: "legacy-app",
                    department_ids: [ @finance.id ] }
    }
    client = AppClient.find_by(client_identifier: "legacy-app")
    assert_empty client.client_department_accesses
  end

  test "uploads a document, then soft-deletes it to the trash" do
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
    assert_not Document.exists?(doc.id)            # hidden from the index
    assert Document.with_deleted.exists?(doc.id)   # retained for restore

    get "/admin/documents"
    assert_not_includes response.body, "report.pdf"
  end

  test "uploads a document with tags" do
    post "/admin/documents",
         params: { document: { department_id: @finance.id,
                               file: fixture_file_upload("report.pdf", "application/pdf"),
                               tags_text: "type=facture\nurgent" } }
    assert_redirected_to "/admin/documents"
    doc = Document.order(:created_at).last
    assert_equal({ "type" => "facture", "urgent" => nil }, doc.tags)
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

  test "bulk_destroy soft-deletes the selected documents" do
    d1 = build_document(filename: "one.pdf")
    d2 = build_document(filename: "two.pdf")

    post "/admin/documents/bulk_destroy", params: { document_ids: [ d1.id, d2.id ] }

    assert_redirected_to "/admin/documents"
    assert_match(/2 documents moved to trash/, flash[:notice])
    assert_equal 2, Document.deleted.count
    assert_equal 0, Document.count
  end

  test "bulk_destroy with an empty selection redirects with an alert and deletes nothing" do
    doc = build_document(filename: "kept.pdf")

    post "/admin/documents/bulk_destroy", params: {}

    assert_redirected_to "/admin/documents"
    assert flash[:alert].present?
    assert Document.exists?(doc.id)
  end

  test "bulk_download streams a ZIP with id-prefixed entry names and correct bytes" do
    d1 = build_document(filename: "alpha.pdf", bytes: "AAAA")
    d2 = build_document(filename: "beta.pdf", bytes: "BBBB")

    get "/admin/documents/bulk_download", params: { document_ids: [ d1.id, d2.id ] }

    assert_response :success
    assert_equal "application/zip", response.media_type

    entries = read_zip_entries(response.body)
    assert_equal "AAAA", entries.fetch("#{d1.id}-alpha.pdf")
    assert_equal "BBBB", entries.fetch("#{d2.id}-beta.pdf")
  end

  test "bulk_download dedupes same-filename documents by id prefix" do
    d1 = build_document(filename: "report.pdf", bytes: "AAAA")
    d2 = build_document(filename: "report.pdf", bytes: "BBBB")

    get "/admin/documents/bulk_download", params: { document_ids: [ d1.id, d2.id ] }

    assert_response :success
    entries = read_zip_entries(response.body)
    assert_equal 2, entries.size
    assert_equal "AAAA", entries.fetch("#{d1.id}-report.pdf")
    assert_equal "BBBB", entries.fetch("#{d2.id}-report.pdf")
  end

  private

  def build_document(filename:, bytes: "PDF!")
    doc = Document.new(department: @finance, filename: filename, uploaded_by: "seed")
    doc.file.attach(io: StringIO.new(bytes), filename: filename, content_type: "application/pdf")
    doc.save!
    doc
  end

  def read_zip_entries(zip_bytes)
    entries = {}
    Zip::File.open_buffer(StringIO.new(zip_bytes)) do |zip|
      zip.each { |entry| entries[entry.name] = entry.get_input_stream.read }
    end
    entries
  end
end
