require "test_helper"

class DiskSpaceTest < ActiveSupport::TestCase
  test "reports total and available space for an existing path" do
    result = DiskSpace.for(Rails.root)
    assert_operator result.total_kb, :>, 0
    assert_operator result.available_kb, :>, 0
    assert_includes 0.0..1.0, result.used_fraction
  end

  test "returns nil for a nonexistent path" do
    assert_nil DiskSpace.for(Rails.root.join("no-such-dir-anywhere"))
  end
end
