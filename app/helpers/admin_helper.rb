module AdminHelper
  STATUS_BADGE = {
    "completed" => "ok",
    "processing" => "info",
    "pending" => "warn",
    "failed" => "bad"
  }.freeze

  # CSS modifier for an upload request status badge (see application.css .badge).
  def status_badge_class(status)
    STATUS_BADGE.fetch(status.to_s, "")
  end
end
