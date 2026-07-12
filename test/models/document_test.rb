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
end
