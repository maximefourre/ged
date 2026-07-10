class Admin::DocumentsController < Admin::BaseController
  include DocumentStreaming

  def index
    scope = Document.includes(:department, :tags).order(created_at: :desc, id: :desc)
    if params[:filename].present?
      scope = scope.where("filename ILIKE ?",
                          "%#{Document.sanitize_sql_like(params[:filename])}%")
    end
    scope = scope.where(department_id: params[:department_id]) if params[:department_id].present?
    @documents = paginate(scope)
  end

  def new
    @document = Document.new
  end

  def create
    file = params.require(:document)[:file]
    if file.blank?
      redirect_to new_admin_document_path, alert: "File is required." and return
    end
    unless file.respond_to?(:original_filename)
      redirect_to new_admin_document_path, alert: "File must be a file upload." and return
    end
    DocumentCreator.call(
      department_id: params.require(:document)[:department_id],
      io: file.tempfile,
      filename: file.original_filename,
      content_type: file.content_type,
      byte_size: file.size,
      uploaded_by: "admin:#{current_admin.username}"
    )
    redirect_to admin_documents_path, notice: "Document uploaded."
  rescue ActiveRecord::RecordInvalid => e
    redirect_to new_admin_document_path,
                alert: e.record.errors.full_messages.join(", ")
  end

  def edit
    @document = Document.find(params[:id])
  end

  def update
    document = Document.find(params[:id])
    document.update!(tag_ids: params.require(:document).permit(tag_ids: [])[:tag_ids])
    redirect_to admin_documents_path, notice: "Tags updated."
  end

  def download
    send_document_file(Document.find(params[:id]))
  end

  def destroy
    Document.find(params[:id]).destroy
    redirect_to admin_documents_path, notice: "Document deleted."
  end
end
