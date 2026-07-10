module Api
  module V1
    class DocumentTagsController < BaseController
      def create
        document = Document.find(params[:document_id])
        authorize_department!(document.department)
        tag = Tag.find_by!(name: require_scalar_param(:name).strip.downcase)
        begin
          document.tags << tag
        rescue ActiveRecord::RecordNotUnique
          # already assigned (repeat call or concurrent request) — idempotent
        end
        render json: { document_id: document.id, tags: document.tags.pluck(:name) },
               status: :created
      end
    end
  end
end
