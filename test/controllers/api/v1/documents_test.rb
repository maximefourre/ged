require "test_helper"

class Api::V1::DocumentsTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @invoices = Department.create!(name: "Invoices", parent: @finance)
    @hr = Department.create!(name: "HR")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance
  end

  def upload_file
    fixture_file_upload("report.pdf", "application/pdf")
  end

  test "sync upload returns 201 with the document uuid" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id, file: upload_file }
    assert_response :created
    body = JSON.parse(response.body)
    assert_match(/\A[0-9a-f-]{36}\z/, body["id"])
    assert_equal "report.pdf", body["filename"]
    assert_equal @finance.id, body["department_id"]
    assert_equal "erp-app", body["uploaded_by"]
    assert Document.find(body["id"]).file.attached?
  end

  test "upload into a subfolder of a granted department is allowed" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @invoices.id, file: upload_file }
    assert_response :created
  end

  test "upload with a non-file file param returns 400" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id, file: "hello" }
    assert_response :bad_request
    assert_equal 0, Document.count
  end

  test "upload into a forbidden department returns 403" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @hr.id, file: upload_file }
    assert_response :forbidden
  end

  test "upload without file returns 400" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id }
    assert_response :bad_request
  end

  test "list returns only accessible documents, with filters" do
    doc = create_document(@invoices, "facture-2026.pdf")
    create_document(@hr, "salaries.pdf")
    doc.tags << Tag.create!(name: "invoice")

    get "/api/v1/documents", headers: bearer_header("erp-app")
    names = JSON.parse(response.body).map { |d| d["filename"] }
    assert_equal [ "facture-2026.pdf" ], names

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { filename: "facture" }
    assert_equal 1, JSON.parse(response.body).size

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "invoice" }
    assert_equal 1, JSON.parse(response.body).size

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "nope" }
    assert_equal 0, JSON.parse(response.body).size
  end

  test "tag filter does not hide the document's other tags" do
    doc = create_document(@invoices, "facture-2026.pdf")
    doc.tags << Tag.create!(name: "invoice")
    doc.tags << Tag.create!(name: "archived")

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "invoice" }
    body = JSON.parse(response.body)
    assert_equal 1, body.size
    assert_equal %w[archived invoice], body.first["tags"].sort
  end

  test "list paginates with limit and offset" do
    3.times { |i| create_document(@finance, "doc-#{i}.pdf") }

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { limit: 2 }
    assert_equal 2, JSON.parse(response.body).size

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { limit: 2, offset: 2 }
    assert_equal 1, JSON.parse(response.body).size

    # garbage / non-positive values fall back to the default page size
    [ "abc", 0, -5 ].each do |bad_limit|
      get "/api/v1/documents", headers: bearer_header("erp-app"),
          params: { limit: bad_limit, offset: "junk" }
      assert_equal 3, JSON.parse(response.body).size, "limit=#{bad_limit.inspect}"
    end

    # a limit above the cap is clamped, not an error
    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { limit: 999_999 }
    assert_response :success
    assert_equal 3, JSON.parse(response.body).size
  end

  test "tags and departments lists paginate too" do
    2.times { |i| Tag.create!(name: "tag-#{i}") }
    get "/api/v1/tags", headers: bearer_header("erp-app"), params: { limit: 1 }
    assert_equal 1, JSON.parse(response.body).size

    Department.create!(name: "Ops", parent: @finance).tap { |d| @client.departments << d }
    get "/api/v1/departments", headers: bearer_header("erp-app"), params: { limit: 1 }
    assert_equal 1, JSON.parse(response.body).size
  end

  test "filtering by a forbidden department returns 403" do
    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { department_id: @hr.id }
    assert_response :forbidden
  end

  test "download streams the file content" do
    doc = create_document(@finance, "report.pdf")
    get "/api/v1/documents/#{doc.id}/download", headers: bearer_header("erp-app")
    assert_response :success
    assert_equal "PDF!", response.body
  end

  test "show, download, delete of a forbidden document return 403" do
    doc = create_document(@hr, "salaries.pdf")
    get "/api/v1/documents/#{doc.id}", headers: bearer_header("erp-app")
    assert_response :forbidden
    get "/api/v1/documents/#{doc.id}/download", headers: bearer_header("erp-app")
    assert_response :forbidden
    delete "/api/v1/documents/#{doc.id}", headers: bearer_header("erp-app")
    assert_response :forbidden
  end

  test "delete removes the document" do
    doc = create_document(@finance, "report.pdf")
    delete "/api/v1/documents/#{doc.id}", headers: bearer_header("erp-app")
    assert_response :no_content
    assert_not Document.exists?(doc.id)
  end

  test "unknown document returns 404" do
    get "/api/v1/documents/#{SecureRandom.uuid}", headers: bearer_header("erp-app")
    assert_response :not_found
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
