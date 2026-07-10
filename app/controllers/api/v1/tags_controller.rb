module Api
  module V1
    class TagsController < BaseController
      def index
        render json: paginate(Tag.order(:name)).map { |t| { id: t.id, name: t.name } }
      end

      def create
        name = params.require(:name)
        unless name.is_a?(String)
          return render json: { error: "name must be a string" }, status: :bad_request
        end
        tag = Tag.create!(name: name)
        render json: { id: tag.id, name: tag.name }, status: :created
      rescue ActiveRecord::RecordNotUnique
        # unique-index race: a concurrent create slipped past the validation
        render json: { error: "Name has already been taken" },
               status: :unprocessable_entity
      end
    end
  end
end
