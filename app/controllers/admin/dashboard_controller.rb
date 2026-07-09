class Admin::DashboardController < Admin::BaseController
  def index
    redirect_to admin_documents_path
  end
end
