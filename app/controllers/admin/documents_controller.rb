class Admin::DocumentsController < Admin::BaseController
  include DocumentStreaming

  def index
    scope = Document.includes(:department).order(created_at: :desc, id: :desc)
    if params[:filename].present?
      scope = scope.where("filename ILIKE ?",
                          "%#{Document.sanitize_sql_like(params[:filename])}%")
    end
    scope = scope.where(department_id: params[:department_id]) if params[:department_id].present?
    if params[:tag].present? && params[:tag].is_a?(String)
      # ? is the jsonb key-exists operator, not a bind marker
      scope = scope.where("documents.tags ? :key", key: params[:tag].strip.downcase)
    end
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
      uploaded_by: "admin:#{current_admin.username}",
      tags: parse_tags_text(params.require(:document)[:tags_text])
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
    document.update!(tags: parse_tags_text(params.require(:document)[:tags_text]))
    redirect_to admin_documents_path, notice: "Tags updated."
  rescue ActiveRecord::RecordInvalid => e
    redirect_to edit_admin_document_path(document),
                alert: e.record.errors.full_messages.join(", ")
  end

  def download
    send_document_file(Document.find(params[:id]))
  end

  def destroy
    Document.find(params[:id]).soft_delete!
    redirect_to admin_documents_path, notice: "Document moved to trash."
  end

  private

  # One "key=value" pair per line; a line without "=" is a key with no value.
  def parse_tags_text(text)
    text.to_s.each_line.map(&:strip).reject(&:empty?).to_h do |line|
      key, value = line.split("=", 2)
      [ key, value ]
    end
  end
end
