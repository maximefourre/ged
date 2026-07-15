require "test_helper"

# White-box regression suite from the 2026-07-15 pentest pass
# (docs/security/pentest-2026-07-15.md). Locks in cross-department IDOR
# defenses: department-scoped access must never leak or reach a document
# outside the granted subtree, and read/write must stay independent.
class AuthzIdorTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!

    @dept_a = Department.create!(name: "A")
    @dept_a_child = Department.create!(name: "A-child", parent: @dept_a)
    @dept_b = Department.create!(name: "B")

    @dept_a_client = AppClient.create!(client_identifier: "deptA")
    @dept_a_client.client_department_accesses.create!(
      department: @dept_a, can_read: true, can_write: true
    )

    @reader = AppClient.create!(client_identifier: "reader")
    @reader.client_department_accesses.create!(
      department: @dept_a, can_read: true, can_write: false
    )

    @writer = AppClient.create!(client_identifier: "writer")
    @writer.client_department_accesses.create!(
      department: @dept_a, can_read: false, can_write: true
    )

    @noaccess = AppClient.create!(client_identifier: "noaccess")

    @child_only_client = AppClient.create!(client_identifier: "childOnly")
    @child_only_client.client_department_accesses.create!(
      department: @dept_a_child, can_read: true, can_write: true
    )

    @doc_a = create_document(@dept_a, "a-report.pdf")
    @doc_b = create_document(@dept_b, "b-report.pdf")
  end

  test "deptA client cannot show a document in dept B" do
    get "/api/v1/documents/#{@doc_b.id}", headers: bearer_header("deptA")
    assert_response :forbidden
  end

  test "deptA client cannot download a document in dept B" do
    get "/api/v1/documents/#{@doc_b.id}/download", headers: bearer_header("deptA")
    assert_response :forbidden
  end

  test "deptA client cannot delete a document in dept B" do
    delete "/api/v1/documents/#{@doc_b.id}", headers: bearer_header("deptA")
    assert_response :forbidden
    assert_not @doc_b.reload.deleted_at?
  end

  test "deptA client cannot replace tags on a document in dept B" do
    put "/api/v1/documents/#{@doc_b.id}/tags", headers: bearer_header("deptA"),
        params: { tags: { type: "facture" } }
    assert_response :forbidden
    assert_equal({}, @doc_b.reload.tags)
  end

  test "deptA client cannot filter the list by dept B's department_id" do
    get "/api/v1/documents", headers: bearer_header("deptA"),
        params: { department_id: @dept_b.id }
    assert_response :forbidden
  end

  test "deptA client's unfiltered list never leaks dept B's document" do
    get "/api/v1/documents", headers: bearer_header("deptA")
    assert_response :success
    ids = JSON.parse(response.body).map { |d| d["id"] }
    assert_not_includes ids, @doc_b.id
    assert_includes ids, @doc_a.id
  end

  test "read/write grant on a department cascades to its child (upload)" do
    post "/api/v1/documents", headers: bearer_header("deptA"),
         params: { department_id: @dept_a_child.id, file: upload_file }
    assert_response :created
  end

  test "a grant on a child department never escalates to reach the parent's document" do
    get "/api/v1/documents/#{@doc_a.id}", headers: bearer_header("childOnly")
    assert_response :forbidden
  end

  test "write-only client cannot read (write does not imply read)" do
    get "/api/v1/documents/#{@doc_a.id}", headers: bearer_header("writer")
    assert_response :forbidden
  end

  test "a client with no grants cannot access documents at all" do
    get "/api/v1/documents/#{@doc_a.id}", headers: bearer_header("noaccess")
    assert_response :forbidden

    get "/api/v1/documents", headers: bearer_header("noaccess")
    assert_response :success
    assert_equal [], JSON.parse(response.body)
  end

  test "a random unknown document UUID is 404, never 200 or 500" do
    get "/api/v1/documents/#{SecureRandom.uuid}", headers: bearer_header("deptA")
    assert_response :not_found
  end

  test "deptA client cannot poll an upload request that belongs to dept B" do
    upload_request = UploadRequest.create!(
      department: @dept_b, callback_url: "https://example.com/hook",
      filename: "b-report.pdf", content_type: "application/pdf", uploaded_by: "seed"
    )

    get "/api/v1/upload_requests/#{upload_request.id}", headers: bearer_header("deptA")
    assert_response :forbidden
  end

  private

  def upload_file
    fixture_file_upload("report.pdf", "application/pdf")
  end

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
