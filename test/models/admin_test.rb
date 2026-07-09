require "test_helper"

class AdminTest < ActiveSupport::TestCase
  test "requires unique username" do
    Admin.create!(username: "alice", password: "secret123")
    dup = Admin.new(username: "ALICE", password: "secret123")
    assert_not dup.valid?
  end

  test "authenticates with valid password" do
    admin = Admin.create!(username: "bob", password: "secret123")
    assert admin.valid_password?("secret123")
    assert_not admin.valid_password?("wrong")
  end
end
