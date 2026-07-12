class Document < ApplicationRecord
  include KvTaggable

  # Grace period between soft-deleting a document and PurgeDeletedDocumentsJob
  # hard-deleting it (row + attached file).
  RETENTION = 30.days

  belongs_to :department
  has_one_attached :file

  validates :filename, :uploaded_by, presence: true

  # Sync uploads and the admin UI leave uploaded_at unset; it then coincides
  # with the indexation time. StoreDocumentJob passes the original request time.
  before_validation :ensure_uploaded_at, on: :create

  # Soft-deleted documents are hidden from every read path (API + admin). The
  # escape-hatch scopes below (used by the Trash view and the purge job) opt
  # back in via `unscope`.
  default_scope { where(deleted_at: nil) }
  scope :with_deleted, -> { unscope(where: :deleted_at) }
  scope :deleted,      -> { with_deleted.where.not(deleted_at: nil) }
  scope :purgeable,    -> { with_deleted.where(purge_at: ..Time.current) }

  # "sync" for inline uploads, "async" for documents stored by StoreDocumentJob.
  def upload_mode
    stored_async? ? "async" : "sync"
  end

  # Readonly system metadata surfaced in the API `metadata` block. The
  # indexation date is the top-level created_at; the owner is uploaded_by.
  def metadata_hash
    { uploaded_at: uploaded_at, updated_at: updated_at, upload_mode: upload_mode }
  end

  def soft_delete!
    now = Time.current
    update!(deleted_at: now, purge_at: now + RETENTION)
  end

  def restore!
    update!(deleted_at: nil, purge_at: nil)
  end

  private

  def ensure_uploaded_at
    self.uploaded_at ||= Time.current
  end
end
