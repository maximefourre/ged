require "test_helper"

class HealthCheckTest < ActionDispatch::IntegrationTest
  test "GET /up returns 200" do
    get "/up"
    assert_response :success
  end
end
