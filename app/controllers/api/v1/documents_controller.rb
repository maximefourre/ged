module Api
  module V1
    class DocumentsController < BaseController
      include DocumentStreaming
      include DocumentJson

      def index
        scope = Document.where(department_id: current_app_client.accessible_department_ids)
        if (department_id = scalar_param(:department_id)).present?
          department = Department.find(department_id)
          authorize_department!(department)
          scope = scope.where(department_id: department.subtree_ids)
        end
        if (filename = scalar_param(:filename)).present?
          scope = scope.where("filename ILIKE ?",
                              "%#{Document.sanitize_sql_like(filename)}%")
        end
        if (tag = scalar_param(:tag)).present?
          tag = tag.strip.downcase
          if (tag_value = scalar_param(:tag_value)).present?
            scope = scope.where("documents.tags @> ?", { tag => tag_value.strip }.to_json)
          else
            # ? is the jsonb key-exists operator (GIN-served), not a bind marker
            scope = scope.where("documents.tags ? :key", key: tag)
          end
        elsif scalar_param(:tag_value).present?
          raise BadParameter, "tag_value requires tag"
        end
        scope = paginate(scope.order(created_at: :desc, id: :desc))
        render json: scope.map { |d| document_json(d) }
      end

      def create
        department = Department.find(require_scalar_param(:department_id))
        authorize_department!(department)
        file = params.require(:file)
        unless file.respond_to?(:original_filename)
          return render json: { error: "file must be a multipart file upload" },
                        status: :bad_request
        end

        if scalar_param(:callback_url).present?
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
        send_document_file(find_authorized_document)
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
    end
  end
end
