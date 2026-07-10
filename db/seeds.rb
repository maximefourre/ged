# Idempotent (bin/rails db:seed can run at any point in every environment),
# and keeps the admin password in sync with GED_ADMIN_PASSWORD — a plain
# find_or_create_by! would silently ignore a changed env var.

admin = Admin.find_or_initialize_by(username: ENV.fetch("GED_ADMIN_USERNAME", "admin"))
password =
  if Rails.env.production?
    ENV.fetch("GED_ADMIN_PASSWORD") # raises KeyError if unset in production
  else
    ENV.fetch("GED_ADMIN_PASSWORD", "changeme") # dev/test (bin/ci seeds in test)
  end
admin.password = password unless admin.persisted? && admin.valid_password?(password)
admin.save!
