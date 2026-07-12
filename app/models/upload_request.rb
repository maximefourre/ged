class UploadRequest < ApplicationRecord
  include KvTaggable

  STATUSES = %w[pending processing completed failed].freeze

  belongs_to :department
  belongs_to :document, optional: true

  validates :status, inclusion: { in: STATUSES }
  validates :callback_url, :filename, :uploaded_by, presence: true
  validate :callback_url_allowed

  private

  # Syntactic SSRF check only (no DNS — fast 422 for the client); the
  # resolution-based barrier runs in DeliverCallbackJob.
  def callback_url_allowed
    return if callback_url.blank?
    if (reason = CallbackUrlPolicy.reject_reason(callback_url))
      errors.add(:callback_url, reason)
    end
  end
end
