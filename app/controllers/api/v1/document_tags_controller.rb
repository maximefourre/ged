module Api
  module V1
    class DocumentTagsController < BaseController
      include DocumentJson

      # Replaces the document's whole tag map (map semantics: the client owns
      # the full map; omitting tags clears it).
      def update
        document = Document.find(params[:document_id])
        authorize_department!(document.department)
        document.update!(tags: tags_param || {})
        render json: document_json(document)
      end
    end
  end
end
