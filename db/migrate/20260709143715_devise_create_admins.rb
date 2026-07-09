# frozen_string_literal: true

class DeviseCreateAdmins < ActiveRecord::Migration[8.1]
  def change
    create_table :admins do |t|
      t.string :username, null: false
      t.string :encrypted_password, null: false, default: ""
      t.timestamps null: false
    end
    add_index :admins, :username, unique: true
  end
end
