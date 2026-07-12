class Document < ApplicationRecord
  include KvTaggable

  belongs_to :department
  has_one_attached :file

  validates :filename, :uploaded_by, presence: true

  # Sync uploads and the admin UI leave uploaded_at unset; it then coincides
  # with the indexation time. StoreDocumentJob passes the original request time.
  before_validation :ensure_uploaded_at, on: :create

  # "sync" for inline uploads, "async" for documents stored by StoreDocumentJob.
  def upload_mode
    stored_async? ? "async" : "sync"
  end

  # Readonly system metadata surfaced in the API `metadata` block. The
  # indexation date is the top-level created_at; the owner is uploaded_by.
  def metadata_hash
    { uploaded_at: uploaded_at, updated_at: updated_at, upload_mode: upload_mode }
  end

  private

  def ensure_uploaded_at
    self.uploaded_at ||= Time.current
  end
end
