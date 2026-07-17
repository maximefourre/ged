class Department < ApplicationRecord
  belongs_to :parent, class_name: "Department", optional: true
  has_many :children, class_name: "Department", foreign_key: :parent_id,
           dependent: :restrict_with_error
  has_many :documents, dependent: :restrict_with_error
  has_many :client_department_accesses, dependent: :destroy
  has_many :upload_requests, dependent: :restrict_with_error

  # Soft-deleted departments are hidden from every read path (API + admin). The
  # escape-hatch scopes below (used by the Trash view and the purge job) opt
  # back in via `unscope`.
  default_scope { where(deleted_at: nil) }
  scope :with_deleted, -> { unscope(where: :deleted_at) }
  scope :deleted,      -> { with_deleted.where.not(deleted_at: nil) }

  validates :name, presence: true, uniqueness: { scope: :parent_id, conditions: -> { with_deleted } }
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

  # Unscoped child check: returns true if this department has no children,
  # including trashed ones.
  def leaf?
    !Department.with_deleted.where(parent_id: id).exists?
  end

  # Soft-deletes the department (hides it) and trashes all of its live documents
  # with a 30-day purge window. Raises if the department is not a leaf (has
  # children). The soft-delete and document trash are atomically committed.
  def soft_delete!
    raise ActiveRecord::RecordNotDestroyed, "Department is not a leaf" unless leaf?
    now = Time.current
    transaction do
      update!(deleted_at: now)
      documents.update_all(deleted_at: now, purge_at: now + Document::RETENTION, updated_at: now)
    end
  end

  # Restores the department (un-hides it) and restores all of its trashed
  # documents, clearing their soft-delete markers. The restore and document
  # restore are atomically committed.
  def restore!
    transaction do
      update!(deleted_at: nil)
      Document.deleted.where(department_id: id).update_all(deleted_at: nil, purge_at: nil, updated_at: Time.current)
    end
  end

  # Un-greys the department (clears deleted_at) without touching documents.
  # Used by the single-document restore path to restore a department
  # without affecting its documents' soft-delete state.
  def clear_soft_delete!
    update!(deleted_at: nil)
  end

  private

  def parent_is_not_self_or_descendant
    return if parent_id.blank? || !persisted?
    errors.add(:parent_id, "cannot be self or a descendant") if subtree_ids.include?(parent_id)
  end
end
