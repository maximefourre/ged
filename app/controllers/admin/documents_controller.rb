class Admin::DocumentsController < Admin::BaseController
  include DocumentStreaming

  def index
    scope = Document.includes(:department).order(created_at: :desc, id: :desc)
    if (filename = scalar_param(:filename)).present?
      scope = scope.where("filename ILIKE ?",
                          "%#{Document.sanitize_sql_like(filename)}%")
    end
    if (department_id = scalar_param(:department_id)).present?
      scope = scope.where(department_id: department_id)
    end
    if (tag = scalar_param(:tag)).present?
      # ? is the jsonb key-exists operator, not a bind marker
      scope = scope.where("documents.tags ? :key", key: tag.strip.downcase)
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

  def preview
    document = Document.find(params[:id])
    response.set_header("X-Content-Type-Options", "nosniff")
    if (csp = preview_csp(document.content_type))
      response.set_header("Content-Security-Policy", csp)
    end
    send_document_file(document, disposition: :inline)
  end

  def destroy
    Document.find(params[:id]).soft_delete!
    redirect_to admin_documents_path, notice: "Document moved to trash."
  end

  private

  def preview_csp(content_type)
    if content_type == "application/pdf"
      # Chrome's built-in PDF viewer refuses to render under a CSP `sandbox`
      # directive (even one carrying allow-* tokens): "This page has been
      # blocked by Chrome". Keep a strict policy WITHOUT that directive —
      # `frame-ancestors 'self'` still bars embedding elsewhere. Safety for the
      # PDF itself comes from nosniff (a spoofed HTML upload is never run as
      # HTML) plus PDFium sandboxing the document from the admin origin.
      "frame-ancestors 'self'"
    else
      "default-src 'none'; img-src 'self'; style-src 'unsafe-inline'; sandbox"
    end
  end

  # One "key=value" pair per line; a line without "=" is a key with no value.
  def parse_tags_text(text)
    text.to_s.each_line.map(&:strip).reject(&:empty?).to_h do |line|
      key, value = line.split("=", 2)
      [ key, value ]
    end
  end
end
