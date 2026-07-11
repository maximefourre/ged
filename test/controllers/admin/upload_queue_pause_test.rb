require "test_helper"

class Admin::UploadQueuePauseTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = Admin.create!(username: "alice", password: "secret123")
    sign_in @admin
    @finance = Department.create!(name: "Finance")
  end

  test "pauses and resumes the uploads queue" do
    post "/admin/upload_queue_pause"
    assert_redirected_to "/admin/upload_requests"
    assert UploadQueue.paused?
    assert SolidQueue::Pause.exists?(queue_name: "uploads")

    delete "/admin/upload_queue_pause"
    assert_redirected_to "/admin/upload_requests"
    assert_not UploadQueue.paused?
  end

  test "the banner reflects queue state and request counts" do
    UploadRequest.create!(department: @finance, filename: "r.pdf",
                          callback_url: "https://caller.test/cb",
                          uploaded_by: "erp-app", status: "pending")

    get "/admin/upload_requests"
    assert_response :success
    assert_includes response.body, "active"
    assert_includes response.body, "1 pending"

    UploadQueue.pause!
    get "/admin/upload_requests"
    assert_includes response.body, "paused"
    assert_includes response.body, "Resume"
  end

  test "requires an admin session" do
    sign_out @admin
    post "/admin/upload_queue_pause"
    assert_redirected_to "/admins/sign_in"
  end
end
