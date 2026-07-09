class CreateDepartments < ActiveRecord::Migration[8.1]
  def change
    create_table :departments do |t|
      t.string :name, null: false
      t.references :parent, foreign_key: { to_table: :departments }
      t.timestamps
    end
    add_index :departments, [ :parent_id, :name ], unique: true
    add_index :departments, :name, unique: true, where: "parent_id IS NULL",
              name: "index_departments_on_name_at_root"
  end
end
