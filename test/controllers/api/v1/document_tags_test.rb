require "test_helper"

class Api::V1::DocumentTagsTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @hr = Department.create!(name: "HR")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance
    @document = create_document(@finance, tags: { "draft" => nil })
  end

  def create_document(department, tags: {})
    doc = Document.new(department: department, filename: "report.pdf",
                       uploaded_by: "erp-app", tags: tags)
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!
    doc
  end

  test "PUT replaces the whole tag map" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("erp-app"),
        params: { tags: { type: "facture", annee: "2026" } }
    assert_response :success
    assert_equal({ "type" => "facture", "annee" => "2026" },
                 JSON.parse(response.body)["tags"])
    assert_equal({ "type" => "facture", "annee" => "2026" }, @document.reload.tags)
  end

  test "PUT without tags clears the map" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("erp-app")
    assert_response :success
    assert_equal({}, @document.reload.tags)
  end

  test "keys and values are normalized" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("erp-app"),
        params: { tags: { "  Type " => "  Facture  " } }
    assert_equal({ "type" => "Facture" }, @document.reload.tags)
  end

  test "rejects a non-map tags param with 422 and leaves tags untouched" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("erp-app"),
        params: { tags: "flat" }
    assert_response :unprocessable_entity
    assert_equal({ "draft" => nil }, @document.reload.tags)
  end

  test "rejects nested values with 422" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("erp-app"),
        params: { tags: { type: { nested: "x" } } }
    assert_response :unprocessable_entity
  end

  test "forbidden for a document outside granted departments" do
    other = create_document(@hr)
    put "/api/v1/documents/#{other.id}/tags", headers: bearer_header("erp-app"),
        params: { tags: { type: "facture" } }
    assert_response :forbidden
  end

  test "the tag registry endpoints are gone" do
    # RoutingError is rescuable (config.action_dispatch.show_exceptions =
    # :rescuable in test), so it surfaces as a 404 response rather than a
    # raised exception here.
    get "/api/v1/tags", headers: bearer_header("erp-app")
    assert_response :not_found
  end
end
