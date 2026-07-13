# Key-metrics status page: system health checks plus upload-pipeline and
# document/storage counts. Everything is read from the app DB and the Solid
# Queue tables — SolidQueue models live in a separate queue database, so they
# are counted on their own connection and never SQL-joined to app tables.
class Admin::StatusController < Admin::BaseController
  QUEUES = [ UploadQueue::NAME, "default" ].freeze
  RECENT_FAILURES = 5

  def index
    @health_checks = SystemHealth.checks

    ready_by_queue = SolidQueue::ReadyExecution.group(:queue_name).count
    @queue_depths = QUEUES.index_with { |queue| ready_by_queue.fetch(queue, 0) }
    @requests_by_status = UploadRequest.group(:status).count
    @scheduled_jobs = SolidQueue::ScheduledExecution.count
    @in_flight_jobs = SolidQueue::ClaimedExecution.count
    @failed_jobs = SolidQueue::FailedExecution.count
    @recent_failures = SolidQueue::FailedExecution.includes(:job)
                                                  .order(created_at: :desc)
                                                  .limit(RECENT_FAILURES)
    @queue_paused = UploadQueue.paused?

    @live_documents = Document.count
    @deleted_documents = Document.deleted.count
    @purgeable_documents = Document.purgeable.count
    @total_bytes = ActiveStorage::Blob.sum(:byte_size)
    async_split = Document.group(:stored_async).count
    @sync_documents = async_split.fetch(false, 0)
    @async_documents = async_split.fetch(true, 0)
    @uploads_24h = Document.with_deleted.where(uploaded_at: 24.hours.ago..).count
    @uploads_7d = Document.with_deleted.where(uploaded_at: 7.days.ago..).count
    @top_departments = top_departments_by_volume
  end

  private

  # Live documents' blob bytes + document count per department, top 5 by
  # volume. Grouped sums/counts first, names resolved in a second pass —
  # everything stays in the app DB.
  def top_departments_by_volume
    sums = Document.joins(file_attachment: :blob)
                   .group(:department_id)
                   .sum("active_storage_blobs.byte_size")
    counts = Document.group(:department_id).count
    names = Department.where(id: sums.keys).pluck(:id, :name).to_h
    sums.sort_by { |_id, bytes| -bytes }
        .first(5)
        .map { |id, bytes| [ names[id], bytes, counts.fetch(id, 0) ] }
  end
end
