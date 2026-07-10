module Api
  module V1
    class DocumentsController < BaseController
      def index
        scope = Document.where(department_id: current_app_client.accessible_department_ids)
        if params[:department_id].present?
          department = Department.find(params[:department_id])
          authorize_department!(department)
          scope = scope.where(department_id: department.subtree_ids)
        end
        if params[:filename].present?
          scope = scope.where("filename ILIKE ?",
                              "%#{Document.sanitize_sql_like(params[:filename])}%")
        end
        scope = scope.joins(:tags).where(tags: { name: params[:tag] }) if params[:tag].present?
        scope = paginate(scope.includes(:tags).order(created_at: :desc, id: :desc))
        render json: scope.map { |d| document_json(d) }
      end

      def create
        department = Department.find(params.require(:department_id))
        authorize_department!(department)
        file = params.require(:file)
        unless file.respond_to?(:original_filename)
          return render json: { error: "file must be a multipart file upload" },
                        status: :bad_request
        end

        if params[:callback_url].present?
          return create_async(department, file)
        end

        document = DocumentCreator.call(
          department_id: department.id,
          io: file.tempfile,
          filename: file.original_filename,
          content_type: file.content_type,
          byte_size: file.size,
          uploaded_by: current_app_client.client_identifier
        )
        render json: document_json(document), status: :created
      end

      def show
        document = find_authorized_document
        render json: document_json(document)
      end

      def download
        document = find_authorized_document
        send_data document.file.download,
                  filename: document.filename,
                  content_type: document.content_type || "application/octet-stream"
      end

      def destroy
        document = find_authorized_document
        document.destroy!
        head :no_content
      end

      private

      def create_async(department, file)
        upload_request = UploadRequest.create!(
          department: department,
          callback_url: params[:callback_url],
          filename: file.original_filename,
          content_type: file.content_type,
          uploaded_by: current_app_client.client_identifier
        )
        StagingStore.write(upload_request.id, file.tempfile)
        StoreDocumentJob.perform_later(upload_request.id)
        render json: { request_id: upload_request.id, status: "pending" },
               status: :accepted
      end

      def find_authorized_document
        document = Document.find(params[:id])
        authorize_department!(document.department)
        document
      end

      def document_json(document)
        {
          id: document.id,
          filename: document.filename,
          content_type: document.content_type,
          byte_size: document.byte_size,
          department_id: document.department_id,
          uploaded_by: document.uploaded_by,
          tags: document.tags.map(&:name),
          created_at: document.created_at
        }
      end
    end
  end
end
