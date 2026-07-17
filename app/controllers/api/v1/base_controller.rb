module Api
  module V1
    class BaseController < ActionController::API
      class NotAuthorizedError < StandardError; end
      class BadParameter < StandardError; end

      before_action :authenticate_app_client!

      rescue_from NotAuthorizedError do |e|
        render json: { error: e.message.presence || "forbidden" }, status: :forbidden
      end
      rescue_from BadParameter do |e|
        render json: { error: e.message }, status: :bad_request
      end
      rescue_from ActiveRecord::RecordNotFound do
        render json: { error: "not found" }, status: :not_found
      end
      rescue_from ActionController::ParameterMissing do |e|
        render json: { error: e.message }, status: :bad_request
      end
      rescue_from ActiveRecord::RecordInvalid do |e|
        render json: { error: e.record.errors.full_messages.join(", ") },
               status: :unprocessable_entity
      end

      private

      attr_reader :current_app_client

      def authenticate_app_client!
        token = request.headers["Authorization"]&.match(/\ABearer (.+)\z/)&.captures&.first
        return render_unauthorized("missing bearer token") if token.blank?

        result = Auth::TokenValidator.call(token)
        @current_app_client = AppClient.find_by(client_identifier: result.client_identifier)
        unless @current_app_client
          render json: { error: "client not registered" }, status: :forbidden
        end
      rescue Auth::TokenValidator::InvalidToken => e
        render_unauthorized(e.message)
      end

      def render_unauthorized(message)
        render json: { error: message }, status: :unauthorized
      end

      def authorize_department!(department, mode:)
        unless current_app_client.can_access?(department, mode)
          raise NotAuthorizedError, "no #{mode} access to this department"
        end
      end

      # Crafted non-scalar params (name[]=x, department_id[a]=b) would
      # otherwise reach finder/model code as Array/Parameters and 500 on
      # NoMethodError.
      def scalar_param(key)
        value = params[key]
        unless value.nil? || value.is_a?(String)
          raise BadParameter, "#{key} must be a string"
        end
        value
      end

      def require_scalar_param(key)
        scalar_param(key).presence || raise(ActionController::ParameterMissing.new(key))
      end

      # Identifiers legitimately arrive as JSON integers ({"parent_id": 8}), which
      # Rails does not stringify. Normalize to String so finders behave as they do
      # on the form-encoded path.
      def id_param(key)
        value = params[key]
        unless value.nil? || value.is_a?(String) || value.is_a?(Integer)
          raise BadParameter, "#{key} must be a string or an integer"
        end
        value&.to_s
      end

      # tags arrives as nested params (tags[type]=facture). Anything that is
      # not a nested hash is passed through so model validation rejects it
      # with 422 instead of a 500 here.
      def tags_param
        value = params[:tags]
        value.respond_to?(:to_unsafe_h) ? value.to_unsafe_h : value
      end

      DEFAULT_PAGE_SIZE = 100
      MAX_PAGE_SIZE = 500
      # Cap offset well below Postgres bigint range: an unbounded offset passed
      # straight to .offset(...) overflows int64 (PG::NumericValueOutOfRange ->
      # uncontracted 500) and is a hostile query-planner input long before that.
      MAX_OFFSET = 1_000_000

      # limit/offset pagination for list endpoints: non-positive, non-numeric
      # or non-scalar values fall back to the default, oversized limits and
      # offsets are clamped.
      def paginate(scope)
        requested = int_param(:limit)
        limit = requested.positive? ? [ requested, MAX_PAGE_SIZE ].min : DEFAULT_PAGE_SIZE
        offset = int_param(:offset).clamp(0, MAX_OFFSET)
        scope.limit(limit).offset(offset)
      end

      def int_param(key)
        value = params[key]
        value.is_a?(String) ? value.to_i : 0
      end
    end
  end
end
