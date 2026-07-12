class AddMetadataToDocuments < ActiveRecord::Migration[8.1]
  def up
    add_column :documents, :uploaded_at, :datetime
    add_column :documents, :stored_async, :boolean, default: false, null: false

    # Backfill: uploaded_at defaults to the indexation time (created_at); async
    # documents recover their original upload time and the async flag from the
    # (completed) upload_request that produced them.
    execute <<~SQL
      UPDATE documents SET uploaded_at = created_at WHERE uploaded_at IS NULL
    SQL
    execute <<~SQL
      UPDATE documents d
         SET uploaded_at = ur.created_at, stored_async = true
        FROM upload_requests ur
       WHERE ur.document_id = d.id AND ur.status = 'completed'
    SQL

    change_column_null :documents, :uploaded_at, false

    add_index :documents, :uploaded_by
    add_index :documents, :uploaded_at
    add_index :documents, :created_at
    add_index :documents, :updated_at
    add_index :documents, :stored_async, where: "stored_async", name: "index_documents_async"
  end

  def down
    remove_column :documents, :stored_async
    remove_column :documents, :uploaded_at
  end
end
