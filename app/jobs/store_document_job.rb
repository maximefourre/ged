class StoreDocumentJob < ApplicationJob
  queue_as :uploads

  def perform(upload_request_id)
    upload_request = UploadRequest.find(upload_request_id)
    return unless claim(upload_request)

    begin
      path = StagingStore.path(upload_request.id)
      # One transaction: a crash between the document INSERT and the completed
      # UPDATE would leave a committed document on a still-"processing" request,
      # which reconcile re-enqueues -> duplicate document (at-least-once). With
      # both writes atomic, an aborted run persists nothing and the re-run
      # stores the document exactly once. Active Storage defers the blob's disk
      # write to commit time, so the staging file must stay open until then —
      # the transaction nests inside File.open, not the other way around.
      File.open(path, "rb") do |file|
        ApplicationRecord.transaction do
          document = DocumentCreator.call(
            department_id: upload_request.department_id,
            io: file,
            filename: upload_request.filename,
            content_type: upload_request.content_type,
            byte_size: File.size(path),
            uploaded_by: upload_request.uploaded_by,
            tags: upload_request.tags,
            uploaded_at: upload_request.created_at,
            stored_async: true
          )
          upload_request.update!(status: "completed", document_id: document.id)
        end
      end
    rescue StandardError => e
      Rails.logger.error("Async upload #{upload_request.id} failed: #{e.class}: #{e.message}")
      upload_request.update!(status: "failed", error_message: e.message)
    ensure
      StagingStore.delete(upload_request.id)
      DeliverCallbackJob.perform_later(upload_request.id)
    end
  end

  private

  # Atomic pending -> processing claim: a reconcile re-enqueue racing the
  # original job (queue backlog > STUCK_AFTER) must not store the document
  # twice. A plain status check would let both executions through.
  def claim(upload_request)
    UploadRequest.where(id: upload_request.id, status: "pending")
                 .update_all(status: "processing", updated_at: Time.current) == 1
  end
end
