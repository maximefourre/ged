class Admin::DocumentsController < Admin::BaseController
  def index
    @documents = Document.includes(:department, :tags).order(created_at: :desc)
    if params[:filename].present?
      @documents = @documents.where("filename ILIKE ?",
                                    "%#{Document.sanitize_sql_like(params[:filename])}%")
    end
    @documents = @documents.where(department_id: params[:department_id]) if params[:department_id].present?
  end

  def new
    @document = Document.new
  end

  def create
    file = params.require(:document)[:file]
    if file.blank?
      redirect_to new_admin_document_path, alert: "File is required." and return
    end
    document = Document.new(
      department_id: params.require(:document)[:department_id],
      filename: file.original_filename,
      content_type: file.content_type,
      byte_size: file.size,
      uploaded_by: "admin:#{current_admin.username}"
    )
    document.file.attach(io: file.tempfile, filename: file.original_filename,
                         content_type: file.content_type)
    if document.save
      redirect_to admin_documents_path, notice: "Document uploaded."
    else
      redirect_to new_admin_document_path,
                  alert: document.errors.full_messages.join(", ")
    end
  end

  def download
    document = Document.find(params[:id])
    send_data document.file.download, filename: document.filename,
              content_type: document.content_type || "application/octet-stream"
  end

  def destroy
    Document.find(params[:id]).destroy
    redirect_to admin_documents_path, notice: "Document deleted."
  end
end
