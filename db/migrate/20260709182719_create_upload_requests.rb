class CreateUploadRequests < ActiveRecord::Migration[8.1]
  def change
    create_table :upload_requests, id: :uuid do |t|
      t.string :status, null: false, default: "pending"
      t.string :callback_url, null: false
      t.references :department, null: false, foreign_key: true
      t.string :filename, null: false
      t.string :content_type
      t.string :uploaded_by, null: false
      t.uuid :document_id
      t.string :error_message
      t.timestamps
    end
  end
end
