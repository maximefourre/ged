require "test_helper"

class Admin::DepartmentsControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = Admin.create!(username: "alice", password: "secret123")
    sign_in @admin
  end

  test "destroy on a dept holding LIVE docs redirects to confirm_deletion" do
    dept = Department.create!(name: "Finance")
    doc = Document.create!(
      department_id: dept.id,
      filename: "invoice.pdf",
      uploaded_by: "app"
    )

    delete admin_department_path(dept)

    assert_redirected_to confirm_deletion_admin_department_path(dept)
    assert Department.exists?(dept.id)
  end

  test "destroy on a dept with ALL docs TRASHED redirects to confirm_deletion" do
    dept = Department.create!(name: "Finance")
    doc = Document.create!(
      department_id: dept.id,
      filename: "invoice.pdf",
      uploaded_by: "app"
    )
    doc.soft_delete!

    delete admin_department_path(dept)

    assert_redirected_to confirm_deletion_admin_department_path(dept)
    assert Department.exists?(dept.id)
  end

  test "destroy on a dept with pending upload_requests redirects to confirm_deletion" do
    dept = Department.create!(name: "Finance")
    req = UploadRequest.create!(
      department_id: dept.id,
      callback_url: "https://example.com/callback",
      filename: "pending.pdf",
      uploaded_by: "app"
    )

    delete admin_department_path(dept)

    assert_redirected_to confirm_deletion_admin_department_path(dept)
    assert Department.exists?(dept.id)
  end

  test "destroy on a dept WITH children alerts and doesn't delete" do
    parent = Department.create!(name: "Parent")
    child = Department.create!(name: "Child", parent_id: parent.id)

    delete admin_department_path(parent)

    assert_redirected_to admin_departments_path
    assert_equal "Cannot delete Parent: it has sub-departments. Delete or move them first.",
                 flash[:alert]
    assert Department.exists?(parent.id)
  end

  test "destroy on a dept with trashed child alerts and doesn't delete" do
    parent = Department.create!(name: "Parent")
    child = Department.create!(name: "Child", parent_id: parent.id)
    child.soft_delete!

    delete admin_department_path(parent)

    assert_redirected_to admin_departments_path
    assert_equal "Cannot delete Parent: it has sub-departments. Delete or move them first.",
                 flash[:alert]
    assert Department.exists?(parent.id)
  end

  test "destroy on an empty dept hard-deletes and shows notice" do
    dept = Department.create!(name: "Finance")

    delete admin_department_path(dept)

    assert_redirected_to admin_departments_path
    assert_equal "Department deleted.", flash[:notice]
    assert_not Department.exists?(dept.id)
  end

  test "move_documents reassigns BOTH live and trashed docs AND upload_requests" do
    source = Department.create!(name: "Source")
    target = Department.create!(name: "Target")

    # Create live doc
    live_doc = Document.create!(
      department_id: source.id,
      filename: "live.pdf",
      uploaded_by: "app"
    )

    # Create and trash another doc
    trashed_doc = Document.create!(
      department_id: source.id,
      filename: "trashed.pdf",
      uploaded_by: "app"
    )
    trashed_doc.soft_delete!

    # Create upload request
    req = UploadRequest.create!(
      department_id: source.id,
      callback_url: "https://example.com/callback",
      filename: "pending.pdf",
      uploaded_by: "app"
    )

    post move_documents_admin_department_path(source),
         params: { target_department_id: target.id }

    assert_redirected_to admin_departments_path
    assert_equal "Moved documents to Target and deleted Source.",
                 flash[:notice]

    # Verify source is gone
    assert_not Department.exists?(source.id)

    # Verify docs now belong to target
    live_doc.reload
    assert_equal target.id, live_doc.department_id

    trashed_doc.reload
    assert_equal target.id, trashed_doc.department_id

    # Verify upload request reassigned
    req.reload
    assert_equal target.id, req.department_id
  end

  test "move_documents rejects target == source" do
    source = Department.create!(name: "Source")

    post move_documents_admin_department_path(source),
         params: { target_department_id: source.id }

    assert_redirected_to confirm_deletion_admin_department_path(source)
    assert_equal "Choose a different destination department.", flash[:alert]
    assert Department.exists?(source.id)
  end

  test "move_documents with a trashed target returns 404" do
    source = Department.create!(name: "Source")
    target = Department.create!(name: "Target")
    target.soft_delete!

    post move_documents_admin_department_path(source),
         params: { target_department_id: target.id }

    assert_response :not_found
  end

  test "trash then restore round-trips" do
    dept = Department.create!(name: "Finance")
    doc1 = Document.create!(
      department_id: dept.id,
      filename: "doc1.pdf",
      uploaded_by: "app"
    )
    doc2 = Document.create!(
      department_id: dept.id,
      filename: "doc2.pdf",
      uploaded_by: "app"
    )

    # Trash the department
    post trash_admin_department_path(dept)

    assert_redirected_to admin_departments_path
    assert dept.reload.deleted_at.present?
    assert doc1.reload.deleted_at.present?
    assert doc2.reload.deleted_at.present?

    # Restore the department
    patch restore_admin_department_path(dept)

    assert_redirected_to admin_departments_path
    assert dept.reload.deleted_at.nil?
    assert doc1.reload.deleted_at.nil?
    assert doc2.reload.deleted_at.nil?
  end

  test "index renders Documents count column" do
    dept1 = Department.create!(name: "Finance")
    dept2 = Department.create!(name: "HR")

    Document.create!(department_id: dept1.id, filename: "f1.pdf", uploaded_by: "app")
    Document.create!(department_id: dept1.id, filename: "f2.pdf", uploaded_by: "app")
    Document.create!(department_id: dept2.id, filename: "h1.pdf", uploaded_by: "app")

    get admin_departments_path

    assert_response :success
    assert_includes response.body, "2", "Finance should show 2 documents"
    assert_includes response.body, "1", "HR should show 1 document"
  end

  test "index shows greyed row and Restore button for trashed dept" do
    dept = Department.create!(name: "Finance")
    dept.soft_delete!

    get admin_departments_path

    assert_response :success
    assert_includes response.body, 'class="trashed"'
    assert_includes response.body, "Restore"
  end

  test "index shows an empty state when there are no departments" do
    get admin_departments_path
    assert_response :success
    assert_match "No departments yet.", response.body
  end
end
