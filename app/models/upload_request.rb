class UploadRequest < ApplicationRecord
  STATUSES = %w[pending completed failed].freeze

  belongs_to :department

  validates :status, inclusion: { in: STATUSES }
  validates :callback_url, :filename, :uploaded_by, presence: true
end
