require "test_helper"

class ApiDocsTest < ActionDispatch::IntegrationTest
  # Collapses both Rails (:id) and OpenAPI ({id}) path params to a placeholder,
  # so paths compare by structure rather than parameter naming.
  def normalize(path)
    path.gsub(/:[A-Za-z_]+/, "{}").gsub(/\{[^}]+\}/, "{}")
  end

  test "GET /api/v1/openapi.yaml serves the spec without authentication" do
    get "/api/v1/openapi.yaml"

    assert_response :success
    assert_equal "application/yaml", response.media_type
    spec = YAML.safe_load(response.body, aliases: true)
    assert_match(/\A3\.1\./, spec["openapi"])
  end

  test "GET /api/docs renders the Redoc page pointing at the spec" do
    get "/api/docs"

    assert_response :success
    assert_equal "text/html", response.media_type
    assert_match "redoc", response.body
    assert_match "/api/v1/openapi.yaml", response.body
  end

  test "every routed /api/v1 path is documented in the spec" do
    spec = YAML.safe_load_file(Rails.root.join("docs/api/openapi.yaml"), aliases: true)
    documented = spec["paths"].keys.map { |p| normalize(p) }.to_set

    routed = Rails.application.routes.routes.filter_map do |route|
      path = route.path.spec.to_s.sub(/\(\.:format\)\z/, "")
      next unless path.start_with?("/api/v1/")
      next if path == "/api/v1/openapi.yaml" # the spec-serving route itself

      normalize(path)
    end.to_set

    missing = routed - documented
    assert_empty missing, "undocumented API paths: #{missing.to_a.join(", ")}"
  end

  test "documents list response conforms to envelope schema" do
    spec = YAML.safe_load_file(Rails.root.join("docs/api/openapi.yaml"), aliases: true)

    list_response = spec.dig("paths", "/api/v1/documents", "get", "responses", "200")
    assert list_response, "documents list endpoint missing 200 response"
    schema_ref = list_response.dig("content", "application/json", "schema", "$ref")
    assert_equal "#/components/schemas/DocumentList", schema_ref,
                 "documents list 200 response should $ref DocumentList"

    document_list = spec.dig("components", "schemas", "DocumentList")
    assert document_list, "DocumentList schema not found"
    assert_equal %w[documents pagination], document_list["properties"].keys.sort,
                 "DocumentList should have documents and pagination properties"

    pagination = spec.dig("components", "schemas", "Pagination")
    assert pagination, "Pagination schema not found"
    assert_equal %w[limit offset total], pagination["properties"].keys.sort,
                 "Pagination should have limit, offset, and total properties"
  end
end
