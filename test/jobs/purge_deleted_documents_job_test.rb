require "test_helper"

class PurgeDeletedDocumentsJobTest < ActiveJob::TestCase
  setup do
    @finance = Department.create!(name: "Finance")
  end

  def create_document(filename: "report.pdf")
    doc = Document.new(department: @finance, filename: filename,
                       content_type: "application/pdf", byte_size: 4,
                       uploaded_by: "erp-app")
    doc.file.attach(io: StringIO.new("PDF!"), filename: filename,
                    content_type: "application/pdf")
    doc.save!
    doc
  end

  test "hard-deletes documents past their purge_at and purges their file" do
    due = create_document
    due.soft_delete!
    due.update_columns(purge_at: 1.day.ago)
    blob_id = due.file.blob.id

    perform_enqueued_jobs(only: ActiveStorage::PurgeJob) do
      PurgeDeletedDocumentsJob.perform_now
    end

    assert_not Document.with_deleted.exists?(due.id)
    assert_not ActiveStorage::Blob.exists?(blob_id)
  end

  test "keeps soft-deleted documents still within the retention window" do
    doc = create_document
    doc.soft_delete! # purge_at ~30 days out
    PurgeDeletedDocumentsJob.perform_now
    assert Document.with_deleted.exists?(doc.id)
  end

  test "keeps live documents" do
    doc = create_document
    PurgeDeletedDocumentsJob.perform_now
    assert Document.exists?(doc.id)
  end

  test "hard-deletes a soft-deleted leaf department once its documents are purged" do
    # Create a leaf department with a document
    dept = Department.create!(name: "Sales")
    doc = create_document
    doc.update!(department_id: dept.id)

    # Soft-delete the department and immediately purge its document
    dept.soft_delete!
    doc.update_columns(purge_at: 1.day.ago)

    perform_enqueued_jobs(only: ActiveStorage::PurgeJob) do
      PurgeDeletedDocumentsJob.perform_now
    end

    # Department should be hard-deleted
    assert_not Department.with_deleted.exists?(dept.id)
  end

  test "keeps a soft-deleted department when a trashed document still remains" do
    # Create a leaf department with a document
    dept = Department.create!(name: "HR")
    doc = create_document
    doc.update!(department_id: dept.id)

    # Soft-delete the department, but leave its document in soft-delete (not purgeable)
    dept.soft_delete!

    PurgeDeletedDocumentsJob.perform_now

    # Department should still exist (document not yet purgeable)
    assert Department.with_deleted.exists?(dept.id)
  end

  test "keeps a soft-deleted department when an UploadRequest references it" do
    # Create a leaf department
    dept = Department.create!(name: "Legal")

    # Create an upload request (no document)
    UploadRequest.create!(department: dept, filename: "r.pdf",
                         callback_url: "https://caller.test/cb",
                         uploaded_by: "erp-app", status: "pending")

    # Soft-delete the department
    dept.soft_delete!

    PurgeDeletedDocumentsJob.perform_now

    # Department should still exist (upload request blocks purge)
    assert Department.with_deleted.exists?(dept.id)
  end

  test "never touches active (non-trashed) departments" do
    dept = Department.create!(name: "Marketing")
    doc = create_document
    doc.update!(department_id: dept.id)

    PurgeDeletedDocumentsJob.perform_now

    # Active department should exist
    assert Department.exists?(dept.id)
  end

  test "hard-delete of department also destroys its access grants" do
    dept = Department.create!(name: "Accounting")
    client = AppClient.create!(client_identifier: "erp-app")
    ClientDepartmentAccess.create!(app_client: client, department: dept)

    assert ClientDepartmentAccess.exists?(department_id: dept.id)

    # Soft-delete and purge
    dept.soft_delete!

    perform_enqueued_jobs(only: ActiveStorage::PurgeJob) do
      PurgeDeletedDocumentsJob.perform_now
    end

    # Department should be gone
    assert_not Department.with_deleted.exists?(dept.id)
    # Grants should be gone too
    assert_not ClientDepartmentAccess.exists?(department_id: dept.id)
  end

  test "purges trashed leaf department with live parent" do
    # Create a live parent with a trashed leaf child (no docs)
    parent = Department.create!(name: "Ops")
    child = Department.create!(name: "Sub", parent: parent)
    child.soft_delete!

    PurgeDeletedDocumentsJob.perform_now

    # Child should be purged (trashed leaf with no docs/requests/children)
    assert_not Department.with_deleted.exists?(child.id)
    # Parent should remain live and untouched
    assert Department.exists?(parent.id)
  end

  private

  def make_doc(dept, filename: "d.pdf")
    doc = Document.create!(department: dept, filename: filename,
                          content_type: "application/pdf", byte_size: 4,
                          uploaded_by: "seed")
    doc
  end
end
