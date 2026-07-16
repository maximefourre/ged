class Admin::BaseController < ApplicationController
  before_action :authenticate_admin!
  layout "admin"

  private

  PER_PAGE = 50
  # Cap the page so the derived offset ((@page - 1) * PER_PAGE) stays well below
  # Postgres bigint range. Admin::BaseController has NO rescue_from, so an
  # unbounded page overflows int64 and renders a raw Rails 500. 20_000 * 50
  # keeps the offset under ~1_000_000, matching the API's MAX_OFFSET.
  MAX_PAGE = 20_000

  # page-based pagination for admin index views: fetches one extra record to
  # know whether an older page exists (rendered by admin/shared/_pagination);
  # non-numeric or non-scalar page values fall back to the first page, and the
  # page is clamped so a huge value cannot overflow the bigint offset.
  def paginate(scope)
    page = params[:page].is_a?(String) ? params[:page].to_i : 0
    @page = page.clamp(1, MAX_PAGE)
    records = scope.offset((@page - 1) * PER_PAGE).limit(PER_PAGE + 1).to_a
    @next_page = @page + 1 if records.size > PER_PAGE
    @prev_page = @page - 1 if @page > 1
    records.first(PER_PAGE)
  end

  # Crafted non-scalar query params (filename[]=x, department_id[a]=b) would
  # otherwise reach finder/model code as Array/ActionController::Parameters and
  # 500. Mirror Api::V1::BaseController#scalar_param, but IGNORE rather than
  # raise: Admin::BaseController has no rescue_from, and an HTML index that 400s
  # on a junk query param is worse UX than one that silently ignores it. Do NOT
  # "fix" this into a raising guard.
  def scalar_param(key)
    value = params[key]
    value.is_a?(String) ? value : nil
  end
end
