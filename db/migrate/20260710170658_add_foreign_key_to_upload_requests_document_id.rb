class AddForeignKeyToUploadRequestsDocumentId < ActiveRecord::Migration[8.1]
  def up
    # References left dangling by documents deleted before this FK existed
    execute <<~SQL
      UPDATE upload_requests SET document_id = NULL
      WHERE document_id IS NOT NULL
        AND document_id NOT IN (SELECT id FROM documents)
    SQL
    add_foreign_key :upload_requests, :documents, on_delete: :nullify
  end

  def down
    remove_foreign_key :upload_requests, :documents
  end
end
