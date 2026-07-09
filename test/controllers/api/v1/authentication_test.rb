require "test_helper"

class Api::V1::AuthenticationTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @client = AppClient.create!(client_identifier: "erp-app")
  end

  test "no Authorization header returns 401" do
    get "/api/v1/ping"
    assert_response :unauthorized
  end

  test "garbage token returns 401" do
    get "/api/v1/ping", headers: { "Authorization" => "Bearer nope" }
    assert_response :unauthorized
  end

  test "expired token returns 401" do
    token = issue_token(client_id: "erp-app", exp: 1.hour.ago.to_i)
    get "/api/v1/ping", headers: { "Authorization" => "Bearer #{token}" }
    assert_response :unauthorized
  end

  test "valid token for an unregistered client returns 403" do
    get "/api/v1/ping", headers: bearer_header("unknown-app")
    assert_response :forbidden
  end

  test "valid token for a registered client returns 200" do
    get "/api/v1/ping", headers: bearer_header("erp-app")
    assert_response :success
    assert_equal "erp-app", JSON.parse(response.body)["client"]
  end
end
