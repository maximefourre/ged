class Admin::UploadRequestsController < Admin::BaseController
  def index
    scope = UploadRequest.includes(:department).order(created_at: :desc, id: :desc)
    if (status = scalar_param(:status)).present?
      scope = scope.where(status: status)
    end
    @upload_requests = paginate(scope)
    @queue_paused = UploadQueue.paused?
    @pending_count = UploadRequest.where(status: "pending").count
    @processing_count = UploadRequest.where(status: "processing").count
  end
end
