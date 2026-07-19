require "test_helper"

class Admin::AppClientsControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = Admin.create!(username: "alice", password: "secret123")
    @finance = Department.create!(name: "Finance")
  end

  def create_client_with_access(identifier:)
    client = AppClient.create!(client_identifier: identifier)
    client.client_department_accesses.create!(department: @finance, can_read: true)
    client
  end

  test "requires an admin session" do
    get "/admin/app_clients"
    assert_redirected_to "/admins/sign_in"
  end

  test "index lists clients with their granted department name" do
    sign_in @admin
    create_client_with_access(identifier: "erp-app")

    get "/admin/app_clients"
    assert_response :success
    assert_includes response.body, "erp-app"
    assert_includes response.body, "Finance"
  end

  test "index renders for a client whose granted department is trashed" do
    sign_in @admin
    create_client_with_access(identifier: "erp-app")
    @finance.soft_delete!

    # Must not 500 when the granted department is soft-deleted; the access row
    # outlives the department's visibility (soft-delete keeps the join rows).
    get "/admin/app_clients"
    assert_response :success
    assert_includes response.body, "erp-app"
    assert_includes response.body, "Finance"
    assert_includes response.body, "(trashed)"
  end

  test "edit form surfaces a granted trashed department marked trashed" do
    sign_in @admin
    client = create_client_with_access(identifier: "erp-app")
    @finance.soft_delete!

    # The default-scoped @departments would hide Finance, making the grant
    # invisible and un-manageable; it must appear, flagged as trashed.
    get "/admin/app_clients/#{client.id}/edit"
    assert_response :success
    assert_includes response.body, "Finance"
    assert_includes response.body, "(trashed)"
  end

  test "admin can remove a grant to a trashed department" do
    sign_in @admin
    client = create_client_with_access(identifier: "erp-app")
    @finance.soft_delete!

    patch "/admin/app_clients/#{client.id}", params: {
      app_client: {
        client_identifier: "erp-app",
        access: { @finance.id.to_s => { "read" => "0", "write" => "0" } }
      }
    }
    assert_redirected_to "/admin/app_clients"
    assert_not ClientDepartmentAccess.exists?(app_client_id: client.id,
                                              department_id: @finance.id)
  end

  test "re-saving keeps an untouched grant to a trashed department" do
    sign_in @admin
    client = create_client_with_access(identifier: "erp-app")
    @finance.soft_delete!

    # Leaving the (trashed) grant checked must not raise on the belongs_to
    # presence validation; the row is preserved so a department restore keeps
    # the access.
    patch "/admin/app_clients/#{client.id}", params: {
      app_client: {
        client_identifier: "erp-app",
        access: { @finance.id.to_s => { "read" => "1", "write" => "1" } }
      }
    }
    assert_redirected_to "/admin/app_clients"
    access = ClientDepartmentAccess.find_by(app_client_id: client.id,
                                            department_id: @finance.id)
    assert access.present?
    assert access.can_read?
    assert access.can_write?
  end

  test "index shows an empty state when there are no clients" do
    sign_in @admin
    get admin_app_clients_path
    assert_response :success
    assert_match "No registered clients.", response.body
  end
end
