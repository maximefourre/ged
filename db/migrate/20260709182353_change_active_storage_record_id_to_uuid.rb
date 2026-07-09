class ChangeActiveStorageRecordIdToUuid < ActiveRecord::Migration[8.1]
  # The Active Storage install migration created record_id as bigint, but the
  # only attachable model (Document) uses uuid primary keys, so record ids were
  # silently integer-cast and corrupted. No recoverable data exists yet.
  def up
    execute "DELETE FROM active_storage_attachments"
    execute "DELETE FROM active_storage_blobs"
    change_column :active_storage_attachments, :record_id, :uuid, using: "NULL::uuid"
  end

  def down
    execute "DELETE FROM active_storage_attachments"
    execute "DELETE FROM active_storage_blobs"
    change_column :active_storage_attachments, :record_id, :bigint, using: "NULL::bigint"
  end
end
