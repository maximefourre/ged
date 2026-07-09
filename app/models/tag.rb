class Tag < ApplicationRecord
  has_many :document_tags, dependent: :destroy
  has_many :documents, through: :document_tags

  normalizes :name, with: ->(name) { name.strip.downcase }

  validates :name, presence: true, uniqueness: true
end
