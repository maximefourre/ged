class Admin::DeletedDocumentsController < Admin::BaseController
  def index
    scope = Document.deleted.order(deleted_at: :desc)
    @documents = paginate(scope)
    @departments_by_id = Department.with_deleted
                                   .where(id: @documents.map(&:department_id))
                                   .index_by(&:id)
  end

  def restore
    document = Document.deleted.find(params[:id])
    document.restore!
    department = Department.with_deleted.find(document.department_id)
    department.clear_soft_delete! if department.deleted_at?
    redirect_to admin_deleted_documents_path, notice: "Document restored."
  end
end
