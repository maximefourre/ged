require "test_helper"

class Api::V1::DepartmentsTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @invoices = Department.create!(name: "Invoices", parent: @finance)
    @hr = Department.create!(name: "HR")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance
  end

  test "list returns only accessible departments (subtree included)" do
    get "/api/v1/departments", headers: bearer_header("erp-app")
    assert_response :success
    names = JSON.parse(response.body).map { |d| d["name"] }
    assert_equal [ "Finance", "Invoices" ], names.sort
  end

  test "create a subfolder under an accessible department" do
    post "/api/v1/departments", headers: bearer_header("erp-app"),
         params: { name: "2026", parent_id: @invoices.id }
    assert_response :created
    body = JSON.parse(response.body)
    assert_equal "2026", body["name"]
    assert_equal @invoices.id, body["parent_id"]
  end

  test "create under a forbidden parent returns 403" do
    post "/api/v1/departments", headers: bearer_header("erp-app"),
         params: { name: "Payroll", parent_id: @hr.id }
    assert_response :forbidden
  end

  test "create without parent_id returns 400 (top-level is admin-only)" do
    post "/api/v1/departments", headers: bearer_header("erp-app"),
         params: { name: "Rogue" }
    assert_response :bad_request
  end

  test "duplicate sibling name returns 422" do
    post "/api/v1/departments", headers: bearer_header("erp-app"),
         params: { name: "Invoices", parent_id: @finance.id }
    assert_response :unprocessable_entity
  end
end
