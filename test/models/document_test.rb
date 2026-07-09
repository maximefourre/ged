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
    doc.tags << Tag.create!(name: "invoice")
    assert_equal [ "invoice" ], doc.tags.pluck(:name)
  end
end
