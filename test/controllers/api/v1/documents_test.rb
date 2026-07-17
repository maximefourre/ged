require "test_helper"

class Api::V1::DocumentsTest < ActionDispatch::IntegrationTest
  include DocumentListHelpers

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

  test "documents expose a readonly metadata block" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id, file: upload_file }
    body = JSON.parse(response.body)
    meta = body["metadata"]
    assert_equal %w[uploaded_at updated_at upload_mode].sort, meta.keys.sort
    assert_equal "sync", meta["upload_mode"]
    assert meta["uploaded_at"].present?
    assert meta["updated_at"].present?
    # indexation date stays the top-level created_at; not duplicated in metadata
    assert body["created_at"].present?
    assert_not meta.key?("indexed_at")
  end

  test "upload into a subfolder of a granted department is allowed" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @invoices.id, file: upload_file }
    assert_response :created
  end

  test "sync upload accepts tags and returns them in the map" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id, file: upload_file,
                   tags: { Type: " Facture ", urgent: "" } }
    assert_response :created
    body = JSON.parse(response.body)
    assert_equal({ "type" => "Facture", "urgent" => nil }, body["tags"])
    assert_equal({ "type" => "Facture", "urgent" => nil }, Document.find(body["id"]).tags)
  end

  test "sync upload with a non-map tags param returns 422" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id, file: upload_file, tags: "flat" }
    assert_response :unprocessable_entity
    assert_equal 0, Document.count
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
    doc.update!(tags: { "invoice" => "2026" })

    get "/api/v1/documents", headers: bearer_header("erp-app")
    names = documents_in(response).map { |d| d["filename"] }
    assert_equal [ "facture-2026.pdf" ], names

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { filename: "facture" }
    assert_equal 1, documents_in(response).size

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "invoice" }
    assert_equal 1, documents_in(response).size

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "nope" }
    assert_equal 0, documents_in(response).size
  end

  test "documents carry their tags as a map and tag_value filters on the pair" do
    doc = create_document(@invoices, "facture-2026.pdf")
    doc.update!(tags: { "type" => "facture", "urgent" => nil })

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "type", tag_value: "facture" }
    body = documents_in(response)
    assert_equal 1, body.size
    assert_equal({ "type" => "facture", "urgent" => nil }, body.first["tags"])

    # filter params are normalized like stored tags
    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: " Type ", tag_value: " facture " }
    assert_equal 1, documents_in(response).size

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "type", tag_value: "devis" }
    assert_equal 0, documents_in(response).size
  end

  test "tag_value without tag returns 400" do
    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag_value: "facture" }
    assert_response :bad_request
  end

  test "list paginates with limit and offset" do
    3.times { |i| create_document(@finance, "doc-#{i}.pdf") }

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { limit: 2 }
    assert_equal 2, documents_in(response).size
    # total counts every match, not the page
    assert_equal({ "limit" => 2, "offset" => 0, "total" => 3 }, pagination_in(response))

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { limit: 2, offset: 2 }
    assert_equal 1, documents_in(response).size

    # garbage / non-positive values fall back to the default page size
    [ "abc", 0, -5 ].each do |bad_limit|
      get "/api/v1/documents", headers: bearer_header("erp-app"),
          params: { limit: bad_limit, offset: "junk" }
      assert_equal 3, documents_in(response).size, "limit=#{bad_limit.inspect}"
    end

    # a limit above the cap is clamped, not an error, and the response echoes
    # the clamped value rather than what was asked for
    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { limit: 999_999 }
    assert_response :success
    assert_equal 3, documents_in(response).size
    assert_equal 500, pagination_in(response)["limit"]
  end

  test "the list is a documents envelope carrying the applied window" do
    create_document(@finance, "doc.pdf")
    get "/api/v1/documents", headers: bearer_header("erp-app")
    body = JSON.parse(response.body)
    assert_equal %w[documents pagination], body.keys.sort
    assert_equal({ "limit" => 100, "offset" => 0, "total" => 1 }, body["pagination"])
  end

  test "pagination total reflects the filters, not the whole scope" do
    3.times { |i| create_document(@finance, "match-#{i}.pdf").update!(tags: { "type" => "facture" }) }
    2.times { |i| create_document(@finance, "other-#{i}.pdf") }

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "type", tag_value: "facture", limit: 1 }
    assert_equal 1, documents_in(response).size
    assert_equal 3, pagination_in(response)["total"]
  end

  test "an empty list still carries a pagination block" do
    get "/api/v1/documents", headers: bearer_header("erp-app")
    assert_equal [], documents_in(response)
    assert_equal({ "limit" => 100, "offset" => 0, "total" => 0 }, pagination_in(response))
  end

  test "an offset above the cap is echoed clamped" do
    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { offset: 5_000_000 }
    assert_equal 1_000_000, pagination_in(response)["offset"]
  end

  test "departments list paginates too" do
    Department.create!(name: "Ops", parent: @finance).tap { |d| @client.departments << d }
    get "/api/v1/departments", headers: bearer_header("erp-app"), params: { limit: 1 }
    assert_equal 1, JSON.parse(response.body).size
  end

  test "non-scalar limit and offset fall back to the default page size" do
    3.times { |i| create_document(@finance, "doc-#{i}.pdf") }
    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { limit: [ 2 ], offset: { page: 1 } }
    assert_response :success
    assert_equal 3, documents_in(response).size
  end

  test "non-string filter params return 400 instead of 500" do
    { filename: [ "x" ], tag: [ "invoice" ], tag_value: [ "x" ],
      department_id: { a: "b" } }.each do |key, value|
      get "/api/v1/documents", headers: bearer_header("erp-app"),
          params: { key => value }
      assert_response :bad_request, "filter #{key}"
    end
  end

  test "non-string department_id or callback_url on upload returns 400" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: [ @finance.id ], file: upload_file }
    assert_response :bad_request

    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id, file: upload_file,
                   callback_url: [ "https://caller.test/cb" ] }
    assert_response :bad_request
    assert_equal 0, UploadRequest.count
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

  test "delete soft-deletes the document, hiding it but keeping the file for the retention window" do
    doc = create_document(@finance, "report.pdf")
    blob_id = doc.file.blob.id

    delete "/api/v1/documents/#{doc.id}", headers: bearer_header("erp-app")
    assert_response :no_content

    # hidden from the client: reads return 404
    assert_not Document.exists?(doc.id)
    get "/api/v1/documents/#{doc.id}", headers: bearer_header("erp-app")
    assert_response :not_found

    # but retained for the retention window: row present, flagged, file kept
    soft_deleted = Document.with_deleted.find(doc.id)
    assert soft_deleted.deleted_at.present?
    assert_equal soft_deleted.deleted_at + 30.days, soft_deleted.purge_at
    assert ActiveStorage::Blob.exists?(blob_id)
  end

  test "unknown document returns 404" do
    get "/api/v1/documents/#{SecureRandom.uuid}", headers: bearer_header("erp-app")
    assert_response :not_found
  end

  test "filters documents by owner and upload mode" do
    create_document(@finance, "sync.pdf")
    async = create_document(@finance, "async.pdf")
    async.update_columns(stored_async: true, uploaded_by: "other-app")

    get "/api/v1/documents", headers: bearer_header("erp-app"), params: { upload_mode: "async" }
    assert_equal [ "async.pdf" ], documents_in(response).map { |d| d["filename"] }

    get "/api/v1/documents", headers: bearer_header("erp-app"), params: { upload_mode: "sync" }
    assert_equal [ "sync.pdf" ], documents_in(response).map { |d| d["filename"] }

    get "/api/v1/documents", headers: bearer_header("erp-app"), params: { uploaded_by: "other-app" }
    assert_equal [ "async.pdf" ], documents_in(response).map { |d| d["filename"] }

    get "/api/v1/documents", headers: bearer_header("erp-app"), params: { upload_mode: "sideways" }
    assert_response :bad_request
  end

  test "filters documents by uploaded_at, indexed_at and updated_at ranges" do
    old = create_document(@finance, "old.pdf")
    old.update_columns(uploaded_at: Time.utc(2020, 1, 1), created_at: Time.utc(2020, 1, 1),
                       updated_at: Time.utc(2020, 1, 1))
    recent = create_document(@finance, "recent.pdf")
    recent.update_columns(uploaded_at: Time.utc(2026, 6, 1), created_at: Time.utc(2026, 6, 1),
                          updated_at: Time.utc(2026, 6, 1))
    cutoff = "2025-01-01T00:00:00Z"

    { uploaded_after: "recent.pdf", indexed_after: "recent.pdf", updated_after: "recent.pdf",
      uploaded_before: "old.pdf", indexed_before: "old.pdf", updated_before: "old.pdf" }.each do |param, expected|
      get "/api/v1/documents", headers: bearer_header("erp-app"), params: { param => cutoff }
      assert_equal [ expected ], documents_in(response).map { |d| d["filename"] }, "filter #{param}"
    end

    get "/api/v1/documents", headers: bearer_header("erp-app"), params: { uploaded_after: "not-a-date" }
    assert_response :bad_request
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
