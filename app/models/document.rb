class Document < ApplicationRecord
  include KvTaggable

  # Grace period between soft-deleting a document and PurgeDeletedDocumentsJob
  # hard-deleting it (row + attached file).
  RETENTION = 30.days

  # Maximum upload size in bytes. Enforced at three layers: Thruster edge cap
  # (MAX_REQUEST_BODY in docker-compose.yml), async staging guard in the
  # controller, and model validation. Env-configurable for testing/operations.
  MAX_BYTE_SIZE = ENV.fetch("GED_MAX_UPLOAD_BYTES", 50.megabytes).to_i

  belongs_to :department
  has_one_attached :file

  # Defense-in-depth: strip path separators / traversal / control bytes from any
  # filename before it is persisted (and before DocumentCreator attaches the blob
  # with this normalized value). Rack already basenames multipart uploads, so the
  # HTTP API never hits this, but internal callers (DocumentCreator, jobs) could.
  def filename=(value)
    super(self.class.normalize_filename(value))
  end

  validates :filename, :uploaded_by, presence: true
  validates :byte_size,
            numericality: { less_than_or_equal_to: ->(_document) { MAX_BYTE_SIZE }, allow_nil: true } # proc so the cap tracks the current constant (kept in sync with the controller guard and testable via a swap)

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

  # Bulk counterpart of soft_delete!, mirroring how Department#soft_delete!
  # bulk-trashes its documents. update_all skips callbacks/validations, which
  # is fine here (same two columns as the single-row path). Returns the
  # affected-row count.
  def self.bulk_soft_delete(ids)
    now = Time.current
    where(id: ids).update_all(deleted_at: now, purge_at: now + RETENTION)
  end

  def restore!
    update!(deleted_at: nil, purge_at: nil)
  end

  private

  def ensure_uploaded_at
    self.uploaded_at ||= Time.current
  end

  # Reduce an arbitrary string to a safe basename: normalize backslashes to
  # slashes, drop control bytes (incl. NUL, so File.basename can't raise), take
  # the basename, strip leading dots, and fall back to "unnamed" if nothing
  # legitimate remains. Spaces are preserved on purpose ("mon rapport.pdf").
  def self.normalize_filename(value)
    return value if value.nil?

    name = value.to_s.tr("\\", "/")
    name = name.delete("\u0000-\u001F\u007F")
    name = File.basename(name)
    name = name.sub(/\A\.+/, "")
    name.presence || "unnamed"
  end
end
