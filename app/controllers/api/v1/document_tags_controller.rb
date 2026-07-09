module Api
  module V1
    class DocumentTagsController < BaseController
      def create
        document = Document.find(params[:document_id])
        authorize_department!(document.department)
        tag = Tag.find_by!(name: params.require(:name).strip.downcase)
        document.tags << tag unless document.tags.exists?(tag.id)
        render json: { document_id: document.id, tags: document.tags.pluck(:name) },
               status: :created
      end
    end
  end
end
