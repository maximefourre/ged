class UploadRequest < ApplicationRecord
  include KvTaggable

  STATUSES = %w[pending processing completed failed].freeze

  belongs_to :department
  belongs_to :document, optional: true

  validates :status, inclusion: { in: STATUSES }
  validates :callback_url, :filename, :uploaded_by, presence: true
  validate :callback_url_is_http

  private

  def callback_url_is_http
    return if callback_url.blank?
    uri = URI.parse(callback_url)
    unless uri.is_a?(URI::HTTP) && uri.host.present?
      errors.add(:callback_url, "must be an http(s) URL with a host")
    end
  rescue URI::InvalidURIError
    errors.add(:callback_url, "must be an http(s) URL with a host")
  end
end
