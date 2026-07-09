require "test_helper"

class TagTest < ActiveSupport::TestCase
  test "normalizes name to lowercase and strips whitespace" do
    tag = Tag.create!(name: "  Invoice ")
    assert_equal "invoice", tag.name
  end

  test "name is required and unique after normalization" do
    Tag.create!(name: "invoice")
    assert_not Tag.new(name: "INVOICE").valid?
    assert_not Tag.new(name: "").valid?
  end
end
