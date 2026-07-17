# GET /api/v1/documents returns an envelope, not a bare array:
# { "documents" => [...], "pagination" => { "limit", "offset", "total" } }.
# (GET /api/v1/departments still returns a bare array.)
module DocumentListHelpers
  def documents_in(response)
    JSON.parse(response.body).fetch("documents")
  end

  def pagination_in(response)
    JSON.parse(response.body).fetch("pagination")
  end
end
