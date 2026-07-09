class CreateDocuments < ActiveRecord::Migration[8.1]
  def change
    create_table :documents, id: :uuid do |t|
      t.string :filename, null: false
      t.string :content_type
      t.bigint :byte_size
      t.references :department, null: false, foreign_key: true
      t.string :uploaded_by, null: false
      t.timestamps
    end

    create_table :document_tags do |t|
      t.references :document, null: false, foreign_key: true, type: :uuid
      t.references :tag, null: false, foreign_key: true
      t.timestamps
    end
    add_index :document_tags, [ :document_id, :tag_id ], unique: true
  end
end
