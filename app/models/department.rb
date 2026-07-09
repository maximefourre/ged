class Department < ApplicationRecord
  belongs_to :parent, class_name: "Department", optional: true
  has_many :children, class_name: "Department", foreign_key: :parent_id,
           dependent: :restrict_with_error
  has_many :documents, dependent: :restrict_with_error

  validates :name, presence: true, uniqueness: { scope: :parent_id }
  validate :parent_is_not_self_or_descendant

  def self_and_ancestors
    node, list, seen = self, [], Set.new
    while node && seen.exclude?(node.id)
      list << node
      seen << node.id
      node = node.parent
    end
    list
  end

  def subtree_ids
    ids = [ id ]
    frontier = [ id ]
    while frontier.any?
      frontier = Department.where(parent_id: frontier).pluck(:id)
      ids.concat(frontier)
    end
    ids
  end

  private

  def parent_is_not_self_or_descendant
    return if parent_id.blank? || !persisted?
    errors.add(:parent_id, "cannot be self or a descendant") if subtree_ids.include?(parent_id)
  end
end
