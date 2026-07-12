class Admin::DeletedDocumentsController < Admin::BaseController
  def index
    scope = Document.deleted.includes(:department).order(deleted_at: :desc)
    @documents = paginate(scope)
  end

  def restore
    Document.deleted.find(params[:id]).restore!
    redirect_to admin_deleted_documents_path, notice: "Document restored."
  end
end
