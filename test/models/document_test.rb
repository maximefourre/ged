require "test_helper"

class DocumentTest < ActiveSupport::TestCase
  setup do
    @department = Department.create!(name: "Finance")
  end

  def build_document
    doc = Document.new(department: @department, filename: "report.pdf",
                       content_type: "application/pdf", byte_size: 4,
                       uploaded_by: "client-a")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc
  end

  test "has a uuid primary key and stores a file" do
    doc = build_document
    doc.save!
    assert_match(/\A[0-9a-f-]{36}\z/, doc.id)
    assert doc.file.attached?
  end

  test "requires filename, department, uploaded_by" do
    assert_not Document.new.valid?
    doc = build_document
    doc.filename = nil
    assert_not doc.valid?
  end

  test "can be tagged" do
    doc = build_document
    doc.save!
    doc.update!(tags: { "invoice" => nil })
    assert_equal({ "invoice" => nil }, doc.reload.tags)
  end

  test "defaults uploaded_at to creation time and is sync" do
    doc = build_document
    freeze_time do
      doc.save!
      assert_equal Time.current, doc.uploaded_at
    end
    assert_not doc.stored_async?
    assert_equal "sync", doc.upload_mode
  end

  test "keeps an explicitly provided uploaded_at and async flag" do
    at = 2.hours.ago.change(usec: 0)
    doc = build_document
    doc.uploaded_at = at
    doc.stored_async = true
    doc.save!
    doc.reload
    assert_equal at, doc.uploaded_at
    assert doc.stored_async?
    assert_equal "async", doc.upload_mode
  end

  test "metadata_hash exposes uploaded_at, updated_at and upload_mode" do
    doc = build_document
    doc.save!
    meta = doc.metadata_hash
    assert_equal %i[uploaded_at updated_at upload_mode].sort, meta.keys.sort
    assert_equal doc.uploaded_at, meta[:uploaded_at]
    assert_equal doc.updated_at, meta[:updated_at]
    assert_equal "sync", meta[:upload_mode]
  end

  test "soft_delete! records deleted_at and a purge_at 30 days later" do
    doc = build_document
    doc.save!
    doc.soft_delete!
    assert doc.deleted_at.present?
    assert_equal doc.deleted_at + 30.days, doc.purge_at
  end

  test "default scope hides soft-deleted documents" do
    doc = build_document
    doc.save!
    doc.soft_delete!
    assert_equal [], Document.all.to_a
    assert_not Document.exists?(doc.id)
    assert_raises(ActiveRecord::RecordNotFound) { Document.find(doc.id) }
  end

  test "deleted and with_deleted scopes reach soft-deleted documents" do
    live = build_document
    live.save!
    gone = build_document
    gone.save!
    gone.soft_delete!
    assert_equal [ gone.id ], Document.deleted.pluck(:id)
    assert_equal [ gone.id, live.id ].sort, Document.with_deleted.pluck(:id).sort
  end

  test "purgeable returns only soft-deleted documents past their purge_at" do
    due = build_document
    due.save!
    due.soft_delete!
    due.update_columns(purge_at: 1.day.ago)
    not_due = build_document
    not_due.save!
    not_due.soft_delete!
    live = build_document
    live.save!
    assert_equal [ due.id ], Document.purgeable.pluck(:id)
  end

  test "restore! clears the soft-delete columns and makes the document visible again" do
    doc = build_document
    doc.save!
    doc.soft_delete!
    doc.restore!
    assert_nil doc.deleted_at
    assert_nil doc.purge_at
    assert Document.exists?(doc.id)
  end
end
