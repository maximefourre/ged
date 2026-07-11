class ReplaceTagRegistryWithJsonbTags < ActiveRecord::Migration[8.1]
  def up
    add_column :documents, :tags, :jsonb, null: false, default: {}
    add_column :upload_requests, :tags, :jsonb, null: false, default: {}
    add_index :documents, :tags, using: :gin

    # Each flat tag name becomes a key with a null value.
    execute <<~SQL
      UPDATE documents
      SET tags = agg.map
      FROM (
        SELECT document_tags.document_id, jsonb_object_agg(tags.name, NULL::text) AS map
        FROM document_tags
        JOIN tags ON tags.id = document_tags.tag_id
        GROUP BY document_tags.document_id
      ) agg
      WHERE documents.id = agg.document_id
    SQL

    drop_table :document_tags
    drop_table :tags
  end

  def down
    raise ActiveRecord::IrreversibleMigration
  end
end
