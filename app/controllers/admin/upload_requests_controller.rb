class Admin::UploadRequestsController < Admin::BaseController
  def index
    @upload_requests = UploadRequest.includes(:department).order(created_at: :desc)
    @upload_requests = @upload_requests.where(status: params[:status]) if params[:status].present?
  end
end
