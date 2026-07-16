# Custom exceptions_app (wired via config.exceptions_app in config/application.rb)
# so that errors raised in middleware *before* any controller runs — most
# importantly ActionDispatch::Http::Parameters::ParseError on a malformed JSON
# request body — are rendered as the API's { "error": ... } JSON contract for
# /api/v1 requests, instead of Rails' default static public/4xx.html page. A
# controller `rescue_from` cannot catch these: the exception never reaches a
# controller (ParseError is in ActionDispatch's rescue_responses as
# :bad_request, so ShowExceptions routes it here).
#
# Requests outside /api/v1 fall through to the stock ActionDispatch::PublicExceptions
# behavior (the static HTML error pages).
#
# SECURITY: the rendered message is a generic, status-derived string only. This
# same path also handles unexpected 500s whose exception messages can carry
# sensitive internals (e.g. SQL fragments); the raw exception message is NEVER
# echoed to the client.
class JsonApiExceptions
  def initialize(public_path)
    @public_exceptions = ActionDispatch::PublicExceptions.new(public_path)
  end

  def call(env)
    request = ActionDispatch::Request.new(env)
    original_path = request.get_header("action_dispatch.original_path").to_s

    return @public_exceptions.call(env) unless original_path.start_with?("/api/v1")

    # ShowExceptions rewrites PATH_INFO to "/<status>" before calling us.
    status = request.path_info.delete_prefix("/").to_i
    status = 500 unless status.between?(400, 599)
    body = { error: message_for(status) }.to_json

    [ status,
      { "Content-Type" => "application/json; charset=utf-8",
        "Content-Length" => body.bytesize.to_s },
      [ body ] ]
  end

  private

  # Generic, status-derived text (e.g. "bad request", "not found") — never the
  # raw exception message, which could leak internals on a 500.
  def message_for(status)
    Rack::Utils::HTTP_STATUS_CODES.fetch(status, "error").downcase
  end
end
