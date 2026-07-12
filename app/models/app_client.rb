class AppClient < ApplicationRecord
  has_many :client_department_accesses, dependent: :destroy
  has_many :departments, through: :client_department_accesses

  validates :client_identifier, presence: true, uniqueness: true

  # mode is deliberately required (:read or :write) — a call site that forgets
  # it must fail loudly rather than silently authorize the wrong verb.
  def can_access?(department, mode)
    granted = granted_department_ids(mode)
    department.self_and_ancestors.any? { |dept| granted.include?(dept.id) }
  end

  def accessible_department_ids(mode = :read)
    Department.where(id: granted_department_ids(mode)).flat_map(&:subtree_ids).uniq
  end

  private

  def granted_department_ids(mode)
    column = (mode == :write) ? :can_write : :can_read
    client_department_accesses.where(column => true).pluck(:department_id)
  end
end
