require "test_helper"

class Admin::DeletedDocumentsControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = Admin.create!(username: "alice", password: "secret123")
    @finance = Department.create!(name: "Finance")
  end

  def create_document(filename:, deleted: false)
    doc = Document.new(department: @finance, filename: filename,
                       content_type: "application/pdf", byte_size: 4,
                       uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: filename,
                    content_type: "application/pdf")
    doc.save!
    doc.soft_delete! if deleted
    doc
  end

  test "index lists soft-deleted documents and not live ones" do
    sign_in @admin
    create_document(filename: "gone.pdf", deleted: true)
    create_document(filename: "live.pdf")

    get "/admin/deleted_documents"
    assert_response :success
    assert_includes response.body, "gone.pdf"
    assert_not_includes response.body, "live.pdf"
  end

  test "restore brings a soft-deleted document back into the normal index" do
    sign_in @admin
    doc = create_document(filename: "gone.pdf", deleted: true)

    patch "/admin/deleted_documents/#{doc.id}/restore"
    assert_redirected_to "/admin/deleted_documents"
    assert_nil Document.with_deleted.find(doc.id).deleted_at

    get "/admin/documents"
    assert_includes response.body, "gone.pdf"
  end

  test "requires an admin session" do
    get "/admin/deleted_documents"
    assert_redirected_to "/admins/sign_in"
  end
end
