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
end
