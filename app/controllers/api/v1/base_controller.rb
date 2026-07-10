module Api
  module V1
    class BaseController < ActionController::API
      class NotAuthorizedError < StandardError; end

      before_action :authenticate_app_client!

      rescue_from NotAuthorizedError do |e|
        render json: { error: e.message.presence || "forbidden" }, status: :forbidden
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

      def authorize_department!(department)
        unless current_app_client.can_access?(department)
          raise NotAuthorizedError, "no access to this department"
        end
      end

      DEFAULT_PAGE_SIZE = 100
      MAX_PAGE_SIZE = 500

      # limit/offset pagination for list endpoints: non-positive or non-numeric
      # values fall back to the default, oversized limits are clamped.
      def paginate(scope)
        requested = params[:limit].to_i
        limit = requested.positive? ? [ requested, MAX_PAGE_SIZE ].min : DEFAULT_PAGE_SIZE
        scope.limit(limit).offset([ params[:offset].to_i, 0 ].max)
      end
    end
  end
end
