# Serves the API contract for humans and tooling. Inherits ActionController::Base
# directly (not ApplicationController) so there is no bearer/Devise auth and no
# modern-browser gate: the spec must be reachable by curl, Bruno and the Redoc
# page. Both endpoints are deliberately public — the contract is not secret, and
# the real /api/v1 endpoints stay protected by Api::V1::BaseController.
class ApiDocsController < ActionController::Base
  SPEC_PATH = Rails.root.join("docs/api/openapi.yaml")

  # GET /api/v1/openapi.yaml
  def spec
    render body: File.read(SPEC_PATH), content_type: "application/yaml"
  end

  # GET /api/docs — Redoc renders the spec above. The bundle loads from jsdelivr
  # (major-pinned), so the page needs internet reachable from the viewer's
  # browser; the raw spec endpoint and the Bruno collection do not.
  def show
    render body: <<~HTML, content_type: "text/html"
      <!DOCTYPE html>
      <html>
        <head>
          <title>GED API — Reference</title>
          <meta charset="utf-8"/>
          <meta name="viewport" content="width=device-width, initial-scale=1"/>
          <style>body { margin: 0; padding: 0; }</style>
        </head>
        <body>
          <redoc spec-url="#{api_openapi_spec_path}"></redoc>
          <script src="https://cdn.jsdelivr.net/npm/redoc@2/bundles/redoc.standalone.js"></script>
        </body>
      </html>
    HTML
  end
end
