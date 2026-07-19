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

  test "destroying a department cascades its client access grants" do
    dept = Department.create!(name: "Finance")
    client = AppClient.create!(client_identifier: "erp-app")
    client.departments << dept
    assert dept.destroy
    assert_not ClientDepartmentAccess.exists?(department_id: dept.id)
  end

  test "destroy is blocked when upload requests reference the department" do
    dept = Department.create!(name: "Finance")
    UploadRequest.create!(department: dept, filename: "report.pdf",
                          callback_url: "https://caller.test/cb",
                          uploaded_by: "erp-app", status: "pending")
    assert_not dept.destroy
    assert dept.errors[:base].any?
    assert Department.exists?(dept.id)
  end

  test "parent cannot be self or a descendant" do
    root = Department.create!(name: "Finance")
    child = Department.create!(name: "Invoices", parent: root)
    root.parent = child
    assert_not root.valid?
    child.parent = child
    assert_not child.valid?
  end

  # Helper to create a document with optional soft-delete
  def make_doc(dept, filename: "d.pdf", deleted: false)
    doc = Document.create!(department: dept, filename: filename,
                          content_type: "application/pdf", byte_size: 4,
                          uploaded_by: "seed")
    doc.soft_delete! if deleted
    doc
  end

  test "soft_delete! sets deleted_at on the department and trashes its live documents" do
    dept = Department.create!(name: "Finance")
    doc1 = make_doc(dept, filename: "report.pdf")
    doc2 = make_doc(dept, filename: "invoice.pdf")

    dept.soft_delete!

    dept.reload
    doc1.reload
    doc2.reload

    assert_not_nil dept.deleted_at
    assert_not_nil doc1.deleted_at
    assert_not_nil doc2.deleted_at
    assert_not_nil doc1.purge_at
    assert_not_nil doc2.purge_at
    assert doc1.purge_at > Time.current
    assert doc2.purge_at > Time.current
  end

  test "restore! clears deleted_at and restores all trashed documents" do
    dept = Department.create!(name: "Finance")
    doc1 = make_doc(dept, filename: "report.pdf")
    doc2 = make_doc(dept, filename: "invoice.pdf")

    dept.soft_delete!
    assert_not_nil dept.reload.deleted_at
    assert_not_nil doc1.reload.deleted_at

    dept.restore!

    dept.reload
    doc1.reload
    doc2.reload

    assert_nil dept.deleted_at
    assert_nil doc1.deleted_at
    assert_nil doc2.deleted_at
    assert_nil doc1.purge_at
    assert_nil doc2.purge_at
  end

  test "default_scope hides soft-deleted departments" do
    dept = Department.create!(name: "Finance")
    make_doc(dept)

    # Before soft-delete, department is visible
    assert Department.exists?(dept.id)

    dept.soft_delete!

    # After soft-delete, hidden by default scope
    assert_not Department.exists?(dept.id)

    # But visible with escape-hatch scopes
    assert Department.with_deleted.exists?(dept.id)
    assert Department.deleted.exists?(dept.id)
  end

  test "leaf? returns false when department has a live child" do
    root = Department.create!(name: "Finance")
    child = Department.create!(name: "Invoices", parent: root)

    assert_not root.leaf?
    assert child.leaf?
  end

  test "soft_delete! raises when department has live children" do
    root = Department.create!(name: "Finance")
    child = Department.create!(name: "Invoices", parent: root)

    error = assert_raises(ActiveRecord::RecordNotDestroyed) do
      root.soft_delete!
    end
    assert_match(/not a leaf/i, error.message)
  end

  test "soft_delete! raises when department has trashed children" do
    root = Department.create!(name: "Finance")
    child = Department.create!(name: "Invoices", parent: root)
    make_doc(child)

    child.soft_delete!
    # child is now soft-deleted, but still a child of root

    error = assert_raises(ActiveRecord::RecordNotDestroyed) do
      root.soft_delete!
    end
    assert_match(/not a leaf/i, error.message)
  end

  test "name uniqueness validation allows restoring a trashed department's name" do
    dept1 = Department.create!(name: "Finance")
    make_doc(dept1)

    dept1.soft_delete!

    # Creating a new department with the same name should pass validation
    # (trashed rows are included in uniqueness check via with_deleted scope)
    dept2 = Department.new(name: "Finance")
    # This should NOT raise RecordNotUnique from the DB, but should raise or invalidate
    # because the validation uses with_deleted
    assert_not dept2.valid?
    assert dept2.errors[:name].any?
  end

  test ".tree returns depth-first pairs with depths, filtered sets promoting orphans to roots" do
    a = Department.create!(name: "A")
    b = Department.create!(name: "B", parent: a)
    c = Department.create!(name: "C", parent: b)
    assert_equal [ [ a, 0 ], [ b, 1 ], [ c, 2 ] ], Department.tree
    assert_equal [ [ b, 0 ], [ c, 1 ] ], Department.tree(Department.where.not(id: a.id))
  end

  test "clear_soft_delete! un-greys the department without restoring documents" do
    dept = Department.create!(name: "Finance")
    doc = make_doc(dept)

    dept.soft_delete!
    assert_not_nil dept.reload.deleted_at
    assert_not_nil doc.reload.deleted_at

    dept.clear_soft_delete!

    dept.reload
    doc.reload

    assert_nil dept.deleted_at
    assert_not_nil doc.deleted_at
  end
end
