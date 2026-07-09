module Api
  module V1
    class PingsController < BaseController
      def show
        render json: { client: current_app_client.client_identifier }
      end
    end
  end
end
