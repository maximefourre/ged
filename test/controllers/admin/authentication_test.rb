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
    assert_redirected_to admin_documents_path
    follow_redirect!
    assert_response :success
  end

  test "failed login re-renders the form with the devise alert visible" do
    Admin.create!(username: "alice", password: "secret123")
    post admin_session_path, params: { admin: { username: "alice", password: "wrong" } }
    assert_match "Invalid", response.body
  end
end
