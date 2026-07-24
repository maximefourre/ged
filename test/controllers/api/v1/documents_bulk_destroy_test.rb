require "test_helper"

class Api::V1::DocumentsBulkDestroyTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @invoices = Department.create!(name: "Invoices", parent: @finance)
    @hr = Department.create!(name: "HR")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance # defaults to can_read AND can_write TRUE, cascades to @invoices
  end

  test "authorized batch across a department and its subtree soft-deletes both" do
    d1 = create_document(@finance, "one.pdf")
    d2 = create_document(@invoices, "two.pdf")

    delete "/api/v1/documents/bulk_destroy", headers: bearer_header("erp-app"),
           params: { document_ids: [ d1.id, d2.id ] }

    assert_response :success
    assert_equal({ "deleted" => 2 }, JSON.parse(response.body))

    assert Document.with_deleted.find(d1.id).deleted_at.present?
    assert Document.with_deleted.find(d2.id).deleted_at.present?
    assert_not Document.exists?(d1.id)
    assert_not Document.exists?(d2.id)
  end

  test "a document outside the client's write access forbids the whole batch" do
    granted = create_document(@finance, "granted.pdf")
    forbidden = create_document(@hr, "forbidden.pdf")

    delete "/api/v1/documents/bulk_destroy", headers: bearer_header("erp-app"),
           params: { document_ids: [ granted.id, forbidden.id ] }

    assert_response :forbidden
    assert Document.exists?(granted.id)
    assert Document.exists?(forbidden.id)
  end

  test "an unknown id mixed with a valid id returns 404 and deletes nothing" do
    valid = create_document(@finance, "valid.pdf")

    delete "/api/v1/documents/bulk_destroy", headers: bearer_header("erp-app"),
           params: { document_ids: [ valid.id, SecureRandom.uuid ] }

    assert_response :not_found
    assert Document.exists?(valid.id)
  end

  test "an empty document_ids array returns 400" do
    delete "/api/v1/documents/bulk_destroy", headers: bearer_header("erp-app"),
           params: { document_ids: [] }
    assert_response :bad_request
  end

  test "a missing document_ids param returns 400" do
    delete "/api/v1/documents/bulk_destroy", headers: bearer_header("erp-app")
    assert_response :bad_request
  end

  test "a read-only client is forbidden from bulk-destroying" do
    reader = AppClient.create!(client_identifier: "reader-app")
    reader.client_department_accesses.create!(department: @finance, can_read: true, can_write: false)
    doc = create_document(@finance, "readonly.pdf")

    delete "/api/v1/documents/bulk_destroy", headers: bearer_header("reader-app"),
           params: { document_ids: [ doc.id ] }

    assert_response :forbidden
    assert Document.exists?(doc.id)
  end

  test "more than 500 document_ids returns 400 before any existence check" do
    ids = Array.new(501) { SecureRandom.uuid }

    delete "/api/v1/documents/bulk_destroy", headers: bearer_header("erp-app"),
           params: { document_ids: ids }

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
