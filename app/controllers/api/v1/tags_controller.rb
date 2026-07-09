module Api
  module V1
    class TagsController < BaseController
      def index
        render json: Tag.order(:name).map { |t| { id: t.id, name: t.name } }
      end

      def create
        tag = Tag.create!(name: params.require(:name))
        render json: { id: tag.id, name: tag.name }, status: :created
      end
    end
  end
end
