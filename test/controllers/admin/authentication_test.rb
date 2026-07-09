require "test_helper"

class Admin::AuthenticationTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  test "unauthenticated access to admin area redirects to login" do
    get "/admin"
    assert_redirected_to new_admin_session_path
  end

  test "authenticated admin reaches admin area" do
    admin = Admin.create!(username: "alice", password: "secret123")
    sign_in admin
    get "/admin"
    assert_response :success
  end
end
