require "test_helper"

class SeedsTest < ActiveSupport::TestCase
  def run_seeds
    load Rails.root.join("db", "seeds.rb")
  end

  test "re-seeding updates the admin password when GED_ADMIN_PASSWORD changes" do
    ENV["GED_ADMIN_USERNAME"] = "seeded"
    ENV["GED_ADMIN_PASSWORD"] = "first-password"
    run_seeds
    admin = Admin.find_by!(username: "seeded")
    assert admin.valid_password?("first-password")

    ENV["GED_ADMIN_PASSWORD"] = "second-password"
    run_seeds
    assert admin.reload.valid_password?("second-password")
    assert_not admin.valid_password?("first-password")
  ensure
    ENV.delete("GED_ADMIN_USERNAME")
    ENV.delete("GED_ADMIN_PASSWORD")
  end
end
