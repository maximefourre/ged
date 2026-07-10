require "test_helper"

class Admin::LoginRateLimitTest < ActionDispatch::IntegrationTest
  setup do
    Admins::SessionsController::RATE_LIMIT_STORE.clear
    Admin.create!(username: "alice", password: "secret123")
  end

  test "repeated login attempts are rate limited" do
    10.times do
      post admin_session_path, params: { admin: { username: "alice", password: "wrong" } }
      assert_response :unprocessable_entity
    end

    post admin_session_path, params: { admin: { username: "alice", password: "wrong" } }
    assert_redirected_to new_admin_session_path
    assert_match(/too many login attempts/i, flash[:alert])

    # even the right password is throttled once the limit is hit
    post admin_session_path, params: { admin: { username: "alice", password: "secret123" } }
    assert_redirected_to new_admin_session_path
  end

  test "a few failed attempts do not block a subsequent valid login" do
    3.times do
      post admin_session_path, params: { admin: { username: "alice", password: "wrong" } }
    end
    post admin_session_path, params: { admin: { username: "alice", password: "secret123" } }
    assert_redirected_to admin_root_path
  end
end
