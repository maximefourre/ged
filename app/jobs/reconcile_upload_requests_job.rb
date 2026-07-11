# Recovers async uploads orphaned by a crash/deploy between the 202 and job
# completion, and purges old staging files and terminal upload requests.
# Scheduled via Solid Queue (config/recurring.yml).
class ReconcileUploadRequestsJob < ApplicationJob
  queue_as :default

  STUCK_AFTER = 10.minutes
  RETENTION = 30.days
  ORPHAN_BLOB_AGE = 1.day

  def perform
    recover_stuck_pending_requests
    recover_stuck_processing_requests
    purge_old_terminal_requests
    purge_old_staging_files
    purge_orphaned_blobs
  end

  private

  def recover_stuck_pending_requests
    # A paused uploads queue means pending requests are intentionally
    # waiting; re-enqueueing every run would pile duplicates into it.
    return if UploadQueue.paused?

    UploadRequest.where(status: "pending")
                 .where(created_at: ..STUCK_AFTER.ago).find_each do |request|
      if File.exist?(StagingStore.path(request.id))
        StoreDocumentJob.perform_later(request.id)
      else
        fail_and_notify(request)
      end
    end
  end

  # A worker died mid-job: the claim happened but neither terminal update nor
  # the ensure block ran. With the staging file still on disk the upload can
  # be retried from scratch; without it there is nothing left to store.
  def recover_stuck_processing_requests
    UploadRequest.where(status: "processing")
                 .where(updated_at: ..STUCK_AFTER.ago).find_each do |request|
      if File.exist?(StagingStore.path(request.id))
        request.update!(status: "pending")
        StoreDocumentJob.perform_later(request.id)
      else
        fail_and_notify(request)
      end
    end
  end

  def fail_and_notify(request)
    request.update!(status: "failed",
                    error_message: "staged file lost before processing")
    DeliverCallbackJob.perform_later(request.id)
  end

  def purge_old_terminal_requests
    UploadRequest.where(status: %w[completed failed])
                 .where(created_at: ...RETENTION.ago).find_each do |request|
      StagingStore.delete(request.id)
      request.destroy!
    end
  end

  # Staging files with no live pending request (e.g. rows deleted manually);
  # anything this old is garbage — stuck pending requests are handled above
  # long before the retention window.
  def purge_old_staging_files
    Dir.glob(StagingStore.root.join("*")).each do |path|
      File.delete(path) if File.mtime(path) < RETENTION.ago
    rescue Errno::ENOENT
      # deleted concurrently (e.g. StoreDocumentJob finishing) between the
      # glob and the mtime/delete — already gone, nothing to purge
    end
  end

  # DocumentCreator uploads the blob before save!, so a validation failure
  # leaves an unattached blob and its file on disk. The age guard keeps
  # in-flight attachments (blob created, attach not yet committed) safe.
  def purge_orphaned_blobs
    ActiveStorage::Blob.unattached.where(created_at: ..ORPHAN_BLOB_AGE.ago)
                       .find_each(&:purge_later)
  end
end
