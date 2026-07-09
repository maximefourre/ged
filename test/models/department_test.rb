require "test_helper"

class DepartmentTest < ActiveSupport::TestCase
  test "requires a name, unique among siblings" do
    root = Department.create!(name: "Finance")
    assert_not Department.new(name: "", parent: root).valid?
    assert_not Department.new(name: "Finance").valid?          # duplicate at root
    assert Department.new(name: "Finance", parent: root).valid? # same name under a parent is fine
  end

  test "self_and_ancestors walks up to the root" do
    root = Department.create!(name: "Finance")
    child = Department.create!(name: "Invoices", parent: root)
    grandchild = Department.create!(name: "2026", parent: child)
    assert_equal [ grandchild, child, root ], grandchild.self_and_ancestors
  end

  test "subtree_ids includes self and all descendants" do
    root = Department.create!(name: "Finance")
    child = Department.create!(name: "Invoices", parent: root)
    grandchild = Department.create!(name: "2026", parent: child)
    other = Department.create!(name: "HR")
    assert_equal [ root.id, child.id, grandchild.id ].sort, root.subtree_ids.sort
    assert_equal [ other.id ], other.subtree_ids
  end

  test "parent cannot be self or a descendant" do
    root = Department.create!(name: "Finance")
    child = Department.create!(name: "Invoices", parent: root)
    root.parent = child
    assert_not root.valid?
    child.parent = child
    assert_not child.valid?
  end
end
