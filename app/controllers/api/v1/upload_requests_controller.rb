module Api
  module V1
    class UploadRequestsController < BaseController
      def show
        upload_request = UploadRequest.find(params[:id])
        authorize_department!(upload_request.department, mode: :read)
        render json: {
          request_id: upload_request.id,
          status: upload_request.status,
          document_id: upload_request.document_id,
          error_message: upload_request.error_message,
          tags: upload_request.document&.tags,
          metadata: upload_request.document&.metadata_hash
        }
      end
    end
  end
end
