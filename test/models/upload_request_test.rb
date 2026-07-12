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

  test "rejects a callback_url targeting a private or internal IP literal" do
    finance = Department.create!(name: "Finance")
    request = UploadRequest.new(department: finance, filename: "report.pdf",
                                callback_url: "http://127.0.0.1/cb",
                                uploaded_by: "erp-app")
    assert_not request.valid?
    assert_includes request.errors[:callback_url].join(" "),
                    "private or internal address"
  end

  test "rejects a non-http callback_url" do
    finance = Department.create!(name: "Finance")
    request = UploadRequest.new(department: finance, filename: "report.pdf",
                                callback_url: "ftp://caller.test/cb",
                                uploaded_by: "erp-app")
    assert_not request.valid?
  end

  test "accepts a public hostname callback_url without resolving DNS" do
    finance = Department.create!(name: "Finance")
    stub_dns do
      request = UploadRequest.new(department: finance, filename: "report.pdf",
                                  callback_url: "https://caller.test/cb",
                                  uploaded_by: "erp-app")
      assert request.valid?
    end
  end
end
