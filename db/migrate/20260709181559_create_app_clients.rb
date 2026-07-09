class CreateAppClients < ActiveRecord::Migration[8.1]
  def change
    create_table :app_clients do |t|
      t.string :client_identifier, null: false
      t.string :description
      t.timestamps
    end
    add_index :app_clients, :client_identifier, unique: true

    create_table :client_department_accesses do |t|
      t.references :app_client, null: false, foreign_key: true
      t.references :department, null: false, foreign_key: true
      t.timestamps
    end
    add_index :client_department_accesses, [ :app_client_id, :department_id ],
              unique: true, name: "index_client_department_accesses_uniqueness"
  end
end
