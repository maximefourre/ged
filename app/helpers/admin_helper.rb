module AdminHelper
  STATUS_BADGE = {
    "completed" => "ok",
    "processing" => "info",
    "pending" => "warn",
    "failed" => "bad"
  }.freeze

  HEALTH_BADGE = { ok: "ok", warn: "warn", bad: "bad", unknown: "info" }.freeze

  # CSS modifier for an upload request status badge (see application.css .badge).
  def status_badge_class(status)
    STATUS_BADGE.fetch(status.to_s, "")
  end

  # CSS modifier for a SystemHealth check state badge.
  def health_badge_class(state)
    HEALTH_BADGE.fetch(state, "info")
  end

  # Indented (NBSP-prefixed) select options for a department tree.
  # pairs come from Department.tree; selected is an id or nil.
  def department_tree_options(pairs, selected = nil)
    options_for_select(
      pairs.map { |dept, depth| [ "#{" " * 3 * depth}#{dept.name}", dept.id ] },
      selected
    )
  end
end
