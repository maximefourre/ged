require "test_helper"

class AppClientTest < ActiveSupport::TestCase
  setup do
    @finance = Department.create!(name: "Finance")
    @invoices = Department.create!(name: "Invoices", parent: @finance)
    @hr = Department.create!(name: "HR")
    @client = AppClient.create!(client_identifier: "erp-app", description: "ERP")
    @client.departments << @finance
  end

  test "client_identifier is required and unique" do
    assert_not AppClient.new.valid?
    assert_not AppClient.new(client_identifier: "erp-app").valid?
  end

  test "can_access? covers the granted department and its subtree" do
    assert @client.can_access?(@finance)
    assert @client.can_access?(@invoices)   # subtree
    assert_not @client.can_access?(@hr)
  end

  test "accessible_department_ids expands granted subtrees" do
    assert_equal [ @finance.id, @invoices.id ].sort, @client.accessible_department_ids.sort
  end
end
