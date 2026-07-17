module Api
  module V1
    class DocumentsController < BaseController
      include DocumentStreaming
      include DocumentJson

      def index
        scope = Document.where(department_id: current_app_client.accessible_department_ids(:read))
        if (department_id = scalar_param(:department_id)).present?
          department = Department.find(department_id)
          authorize_department!(department, mode: :read)
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
        scope = filter_by_metadata(scope).order(created_at: :desc, id: :desc)
        render json: paginated_json(scope, key: :documents) { |d| document_json(d) }
      end

      def create
        department = Department.find(require_scalar_param(:department_id))
        authorize_department!(department, mode: :write)
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
          uploaded_by: current_app_client.client_identifier,
          tags: tags_param || {}
        )
        render json: document_json(document), status: :created
      end

      def show
        document = find_authorized_document(mode: :read)
        render json: document_json(document)
      end

      def download
        send_document_file(find_authorized_document(mode: :read))
      end

      def destroy
        document = find_authorized_document(mode: :write)
        document.soft_delete!
        head :no_content
      end

      private

      def create_async(department, file)
        if file.size > Document::MAX_BYTE_SIZE
          return render json: { error: "file is too large (maximum #{Document::MAX_BYTE_SIZE} bytes)" },
                        status: :unprocessable_entity
        end

        upload_request = UploadRequest.create!(
          department: department,
          callback_url: params[:callback_url],
          filename: file.original_filename,
          content_type: file.content_type,
          uploaded_by: current_app_client.client_identifier,
          tags: tags_param || {}
        )
        StagingStore.write(upload_request.id, file.tempfile)
        StoreDocumentJob.perform_later(upload_request.id)
        render json: { request_id: upload_request.id, status: "pending" },
               status: :accepted
      end

      def find_authorized_document(mode:)
        document = Document.find(params[:id])
        authorize_department!(document.department, mode: mode)
        document
      end

      # Readonly-metadata filters: owner (uploaded_by), sync/async mode, and
      # after/before ranges over the uploaded/indexed/updated timestamps.
      def filter_by_metadata(scope)
        if (owner = scalar_param(:uploaded_by)).present?
          scope = scope.where(uploaded_by: owner)
        end
        if (mode = scalar_param(:upload_mode)).present?
          case mode
          when "sync"  then scope = scope.where(stored_async: false)
          when "async" then scope = scope.where(stored_async: true)
          else raise BadParameter, "upload_mode must be sync or async"
          end
        end
        scope = apply_time_range(scope, :uploaded_at, :uploaded_after, :uploaded_before)
        scope = apply_time_range(scope, :created_at, :indexed_after, :indexed_before)
        apply_time_range(scope, :updated_at, :updated_after, :updated_before)
      end

      # Beginless/endless ranges keep the column an identifier (no string
      # interpolation): where(col => t..) => col >= t, where(col => ..t) => col <= t.
      def apply_time_range(scope, column, after_key, before_key)
        if (after = scalar_param(after_key)).present?
          scope = scope.where(column => parse_time(after_key, after)..)
        end
        if (before = scalar_param(before_key)).present?
          scope = scope.where(column => ..parse_time(before_key, before))
        end
        scope
      end

      def parse_time(key, value)
        Time.iso8601(value)
      rescue ArgumentError
        raise BadParameter, "#{key} must be an ISO8601 datetime"
      end
    end
  end
end
