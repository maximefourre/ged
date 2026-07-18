class Admin::UploadRequestsController < Admin::BaseController
  def index
    scope = UploadRequest.order(created_at: :desc, id: :desc)
    if (status = scalar_param(:status)).present?
      scope = scope.where(status: status)
    end
    @upload_requests = paginate(scope)
    # Resolve departments via with_deleted: a soft-deleted (trashed) department
    # is hidden by Department's default_scope, so the belongs_to association
    # would read nil and the view would 500 on .name.
    @departments_by_id = Department.with_deleted
                                   .where(id: @upload_requests.map(&:department_id))
                                   .index_by(&:id)
    @queue_paused = UploadQueue.paused?
    @pending_count = UploadRequest.where(status: "pending").count
    @processing_count = UploadRequest.where(status: "processing").count
  end
end
