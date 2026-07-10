require "test_helper"

class UploadRequestTest < ActiveSupport::TestCase
  test "deleting a document nulls the referencing upload_request's document_id" do
    finance = Department.create!(name: "Finance")
    doc = Document.new(department: finance, filename: "report.pdf",
                       content_type: "application/pdf", byte_size: 4,
                       uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!
    request = UploadRequest.create!(department: finance, filename: "report.pdf",
                                    callback_url: "https://caller.test/cb",
                                    uploaded_by: "erp-app", status: "completed",
                                    document_id: doc.id)
    doc.destroy!
    assert_nil request.reload.document_id
  end
end
