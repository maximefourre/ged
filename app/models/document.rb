class Document < ApplicationRecord
  include KvTaggable

  belongs_to :department
  has_one_attached :file

  validates :filename, :uploaded_by, presence: true
end
