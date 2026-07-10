class Admin::UploadRequestsController < Admin::BaseController
  def index
    scope = UploadRequest.includes(:department).order(created_at: :desc, id: :desc)
    scope = scope.where(status: params[:status]) if params[:status].present?
    @upload_requests = paginate(scope)
  end
end
