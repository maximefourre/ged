class Document < ApplicationRecord
  belongs_to :department
  has_one_attached :file
  has_many :document_tags, dependent: :destroy
  has_many :tags, through: :document_tags

  validates :filename, :uploaded_by, presence: true
end
