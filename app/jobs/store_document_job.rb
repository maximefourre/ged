class StoreDocumentJob < ApplicationJob
  queue_as :default

  def perform(upload_request_id)
    upload_request = UploadRequest.find(upload_request_id)
    return unless upload_request.status == "pending"

    begin
      path = StagingStore.path(upload_request.id)
      document = File.open(path, "rb") do |file|
        DocumentCreator.call(
          department_id: upload_request.department_id,
          io: file,
          filename: upload_request.filename,
          content_type: upload_request.content_type,
          byte_size: File.size(path),
          uploaded_by: upload_request.uploaded_by
        )
      end
      upload_request.update!(status: "completed", document_id: document.id)
    rescue StandardError => e
      Rails.logger.error("Async upload #{upload_request.id} failed: #{e.class}: #{e.message}")
      upload_request.update!(status: "failed", error_message: e.message)
    ensure
      StagingStore.delete(upload_request.id)
      DeliverCallbackJob.perform_later(upload_request.id)
    end
  end
end
