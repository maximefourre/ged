class AddReadWriteToClientDepartmentAccesses < ActiveRecord::Migration[8.1]
  def up
    # Split the binary department grant into independent read/write permissions.
    # Existing grants were full access, so the true/true default backfills them.
    add_column :client_department_accesses, :can_read, :boolean, default: true, null: false
    add_column :client_department_accesses, :can_write, :boolean, default: true, null: false

    # A grant of neither permission is meaningless — "no access" is the absence
    # of a row, not a row with both flags off.
    add_check_constraint :client_department_accesses, "can_read OR can_write",
                         name: "at_least_one_permission"
  end

  def down
    remove_check_constraint :client_department_accesses, name: "at_least_one_permission"
    remove_column :client_department_accesses, :can_write
    remove_column :client_department_accesses, :can_read
  end
end
