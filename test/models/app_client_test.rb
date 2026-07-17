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

  test "can_access? covers the granted department and its subtree for both modes" do
    assert @client.can_access?(@finance, :read)
    assert @client.can_access?(@invoices, :read)   # subtree
    assert @client.can_access?(@finance, :write)
    assert @client.can_access?(@invoices, :write)  # subtree
    assert_not @client.can_access?(@hr, :read)
    assert_not @client.can_access?(@hr, :write)
  end

  test "a read-only grant does not allow write" do
    reader = AppClient.create!(client_identifier: "reader")
    reader.client_department_accesses.create!(department: @finance, can_read: true, can_write: false)

    assert reader.can_access?(@finance, :read)
    assert_not reader.can_access?(@finance, :write)
    assert_not reader.can_access?(@invoices, :write)  # no write anywhere in the subtree
  end

  test "an unknown mode fails closed instead of falling through to read" do
    reader = AppClient.create!(client_identifier: "reader")
    reader.client_department_accesses.create!(department: @finance, can_read: true, can_write: false)

    # A typo'd / stringified / nil mode must raise, not silently authorize via
    # can_read. The dangerous regression is a read-only client answering "yes".
    [ :wrtie, "write", nil ].each do |bad_mode|
      assert_raises(ArgumentError, "mode #{bad_mode.inspect} should fail closed") do
        reader.can_access?(@finance, bad_mode)
      end
    end
  end

  test "a write-only grant does not allow read" do
    writer = AppClient.create!(client_identifier: "writer")
    writer.client_department_accesses.create!(department: @finance, can_read: false, can_write: true)

    assert writer.can_access?(@finance, :write)
    assert_not writer.can_access?(@finance, :read)
  end

  test "read and write cascade independently, child grants never escalate upward" do
    mixed = AppClient.create!(client_identifier: "mixed")
    mixed.client_department_accesses.create!(department: @finance, can_read: true, can_write: false)
    mixed.client_department_accesses.create!(department: @invoices, can_read: false, can_write: true)

    assert mixed.can_access?(@invoices, :read)      # read cascades from Finance
    assert mixed.can_access?(@invoices, :write)     # direct write grant
    assert_not mixed.can_access?(@finance, :write)  # child write grant does not reach the parent
  end

  test "accessible_department_ids expands granted subtrees per mode" do
    partial = AppClient.create!(client_identifier: "partial")
    partial.client_department_accesses.create!(department: @finance, can_read: true, can_write: false)
    partial.client_department_accesses.create!(department: @hr, can_read: false, can_write: true)

    assert_equal [ @finance.id, @invoices.id ].sort, partial.accessible_department_ids(:read).sort
    assert_equal [ @hr.id ], partial.accessible_department_ids(:write)
  end
end
