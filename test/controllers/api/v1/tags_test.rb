require "test_helper"

class Api::V1::TagsTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance
  end

  test "create and list tags" do
    post "/api/v1/tags", headers: bearer_header("erp-app"), params: { name: "Invoice" }
    assert_response :created
    assert_equal "invoice", JSON.parse(response.body)["name"]

    get "/api/v1/tags", headers: bearer_header("erp-app")
    assert_equal [ "invoice" ], JSON.parse(response.body).map { |t| t["name"] }
  end

  test "duplicate tag returns 422" do
    Tag.create!(name: "invoice")
    post "/api/v1/tags", headers: bearer_header("erp-app"), params: { name: "INVOICE" }
    assert_response :unprocessable_entity
  end

  test "assign an existing tag to an accessible document" do
    doc = Document.new(department: @finance, filename: "report.pdf",
                       uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!
    Tag.create!(name: "invoice")

    post "/api/v1/documents/#{doc.id}/tags", headers: bearer_header("erp-app"),
         params: { name: "invoice" }
    assert_response :created
    assert_equal [ "invoice" ], doc.reload.tags.pluck(:name)

    # idempotent: assigning again neither errors nor duplicates
    post "/api/v1/documents/#{doc.id}/tags", headers: bearer_header("erp-app"),
         params: { name: "invoice" }
    assert_response :created
    assert_equal 1, doc.reload.tags.count
  end

  test "assigning an unknown tag returns 404" do
    doc = Document.new(department: @finance, filename: "report.pdf",
                       uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!
    post "/api/v1/documents/#{doc.id}/tags", headers: bearer_header("erp-app"),
         params: { name: "ghost" }
    assert_response :not_found
  end

  test "assigning a tag to a forbidden document returns 403" do
    hr = Department.create!(name: "HR")
    doc = Document.new(department: hr, filename: "salaries.pdf", uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "salaries.pdf",
                    content_type: "application/pdf")
    doc.save!
    Tag.create!(name: "invoice")
    post "/api/v1/documents/#{doc.id}/tags", headers: bearer_header("erp-app"),
         params: { name: "invoice" }
    assert_response :forbidden
  end
end
