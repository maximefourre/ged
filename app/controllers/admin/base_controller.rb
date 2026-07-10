class Admin::BaseController < ApplicationController
  before_action :authenticate_admin!
  layout "admin"

  private

  PER_PAGE = 50

  # page-based pagination for admin index views: fetches one extra record to
  # know whether an older page exists (rendered by admin/shared/_pagination);
  # non-numeric or non-scalar page values fall back to the first page
  def paginate(scope)
    page = params[:page].is_a?(String) ? params[:page].to_i : 0
    @page = page.positive? ? page : 1
    records = scope.offset((@page - 1) * PER_PAGE).limit(PER_PAGE + 1).to_a
    @next_page = @page + 1 if records.size > PER_PAGE
    @prev_page = @page - 1 if @page > 1
    records.first(PER_PAGE)
  end
end
