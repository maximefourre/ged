require "test_helper"

class ClientDepartmentAccessTest < ActiveSupport::TestCase
  setup do
    @client = AppClient.create!(client_identifier: "erp-app", description: "ERP")
    @finance = Department.create!(name: "Finance")
  end

  test "defaults to full read/write access" do
    access = @client.client_department_accesses.create!(department: @finance)
    assert access.can_read?
    assert access.can_write?
  end

  test "rejects a grant with neither read nor write" do
    access = @client.client_department_accesses.build(
      department: @finance, can_read: false, can_write: false
    )
    assert_not access.valid?
    assert_includes access.errors[:base].join(" "), "read or write"
  end
end
