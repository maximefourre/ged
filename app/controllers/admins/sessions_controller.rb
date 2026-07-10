class Admins::SessionsController < Devise::SessionsController
  # Per-process store: brute-force protection doesn't need cross-worker
  # accuracy, and the test env's null cache store would disable it.
  RATE_LIMIT_STORE = ActiveSupport::Cache::MemoryStore.new

  rate_limit to: 10, within: 3.minutes, only: :create, store: RATE_LIMIT_STORE,
             with: -> {
               redirect_to new_admin_session_path,
                           alert: "Too many login attempts. Try again in a few minutes."
             }
end
