class AddSoftDeleteToDocuments < ActiveRecord::Migration[8.1]
  def change
    add_column :documents, :deleted_at, :datetime
    add_column :documents, :purge_at, :datetime
    add_index :documents, :purge_at
  end
end
