class Admin::UploadQueuePausesController < Admin::BaseController
  def create
    UploadQueue.pause!
    redirect_to admin_upload_requests_path, notice: "Upload queue paused."
  end

  def destroy
    UploadQueue.resume!
    redirect_to admin_upload_requests_path, notice: "Upload queue resumed."
  end
end
