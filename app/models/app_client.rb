class AppClient < ApplicationRecord
  has_many :client_department_accesses, dependent: :destroy
  has_many :departments, through: :client_department_accesses

  validates :client_identifier, presence: true, uniqueness: true

  def can_access?(department)
    granted = department_ids
    department.self_and_ancestors.any? { |dept| granted.include?(dept.id) }
  end

  def accessible_department_ids
    departments.flat_map(&:subtree_ids).uniq
  end
end
