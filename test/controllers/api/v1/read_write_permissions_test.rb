require "test_helper"

# Verb-level authorization matrix: a read-only grant must block every write
# endpoint, a write-only grant must block every read endpoint.
class Api::V1::ReadWritePermissionsTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @invoices = Department.create!(name: "Invoices", parent: @finance)

    @reader = AppClient.create!(client_identifier: "reader-app")
    @reader.client_department_accesses.create!(
      department: @finance, can_read: true, can_write: false
    )

    @writer = AppClient.create!(client_identifier: "writer-app")
    @writer.client_department_accesses.create!(
      department: @finance, can_read: false, can_write: true
    )

    @document = create_document(@finance, "report.pdf")
  end

  def upload_file
    fixture_file_upload("report.pdf", "application/pdf")
  end

  # --- read-only client ---

  test "read-only client can list, show and download" do
    get "/api/v1/documents", headers: bearer_header("reader-app")
    assert_response :success
    assert_equal [ "report.pdf" ], JSON.parse(response.body).map { |d| d["filename"] }

    get "/api/v1/documents/#{@document.id}", headers: bearer_header("reader-app")
    assert_response :success

    get "/api/v1/documents/#{@document.id}/download", headers: bearer_header("reader-app")
    assert_response :success
  end

  test "read-only client cannot upload" do
    post "/api/v1/documents", headers: bearer_header("reader-app"),
         params: { department_id: @finance.id, file: upload_file }
    assert_response :forbidden
    assert_match(/no write access/, JSON.parse(response.body)["error"])
  end

  test "read-only client cannot delete" do
    delete "/api/v1/documents/#{@document.id}", headers: bearer_header("reader-app")
    assert_response :forbidden
    assert_not @document.reload.deleted_at?
  end

  test "read-only client cannot replace tags" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("reader-app"),
        params: { tags: { type: "facture" } }
    assert_response :forbidden
    assert_equal({}, @document.reload.tags)
  end

  test "read-only client cannot create a sub-department" do
    post "/api/v1/departments", headers: bearer_header("reader-app"),
         params: { parent_id: @finance.id, name: "Archives" }
    assert_response :forbidden
  end

  # --- write-only client ---

  test "write-only client can upload and gets the created document back" do
    post "/api/v1/documents", headers: bearer_header("writer-app"),
         params: { department_id: @finance.id, file: upload_file }
    assert_response :created
    assert_equal "report.pdf", JSON.parse(response.body)["filename"]
  end

  test "write-only client cannot list, show or download" do
    get "/api/v1/documents", headers: bearer_header("writer-app")
    assert_response :success
    assert_equal [], JSON.parse(response.body)  # list scope is read-based: empty

    get "/api/v1/documents/#{@document.id}", headers: bearer_header("writer-app")
    assert_response :forbidden
    assert_match(/no read access/, JSON.parse(response.body)["error"])

    get "/api/v1/documents/#{@document.id}/download", headers: bearer_header("writer-app")
    assert_response :forbidden
  end

  test "write-only client cannot filter the list by department" do
    get "/api/v1/documents", headers: bearer_header("writer-app"),
        params: { department_id: @finance.id }
    assert_response :forbidden
  end

  test "write-only client sees no departments in the read-scoped index" do
    get "/api/v1/departments", headers: bearer_header("writer-app")
    assert_response :success
    assert_equal [], JSON.parse(response.body)
  end

  test "write-only client can delete (delete is a write)" do
    delete "/api/v1/documents/#{@document.id}", headers: bearer_header("writer-app")
    assert_response :no_content
    assert @document.reload.deleted_at?
  end

  test "write-only client can replace tags" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("writer-app"),
        params: { tags: { type: "facture" } }
    assert_response :success
    assert_equal({ "type" => "facture" }, @document.reload.tags)
  end

  test "write-only client cannot poll an upload request (poll is a read)" do
    upload_request = UploadRequest.create!(
      department: @finance, callback_url: "https://example.com/hook",
      filename: "report.pdf", content_type: "application/pdf", uploaded_by: "writer-app"
    )

    get "/api/v1/upload_requests/#{upload_request.id}", headers: bearer_header("writer-app")
    assert_response :forbidden

    get "/api/v1/upload_requests/#{upload_request.id}", headers: bearer_header("reader-app")
    assert_response :success
  end

  # Regression guard: show/download and destroy share find_authorized_document —
  # the mode split must not let a read grant reach the write verb.
  test "destroy requires write even though it shares the finder with show" do
    delete "/api/v1/documents/#{@document.id}", headers: bearer_header("reader-app")
    assert_response :forbidden

    get "/api/v1/documents/#{@document.id}", headers: bearer_header("reader-app")
    assert_response :success
  end

  private

  def create_document(department, filename)
    doc = Document.new(department: department, filename: filename,
                       content_type: "application/pdf", byte_size: 4,
                       uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: filename,
                    content_type: "application/pdf")
    doc.save!
    doc
  end
end
