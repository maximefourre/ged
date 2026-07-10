# Recovers async uploads orphaned by a crash/deploy between the 202 and job
# completion, and purges old staging files and terminal upload requests.
# Scheduled via Solid Queue (config/recurring.yml).
class ReconcileUploadRequestsJob < ApplicationJob
  queue_as :default

  STUCK_AFTER = 10.minutes
  RETENTION = 30.days

  def perform
    recover_stuck_pending_requests
    purge_old_terminal_requests
    purge_old_staging_files
  end

  private

  def recover_stuck_pending_requests
    UploadRequest.where(status: "pending")
                 .where(created_at: ..STUCK_AFTER.ago).find_each do |request|
      if File.exist?(StagingStore.path(request.id))
        StoreDocumentJob.perform_later(request.id)
      else
        request.update!(status: "failed",
                        error_message: "staged file lost before processing")
        DeliverCallbackJob.perform_later(request.id)
      end
    end
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
end
