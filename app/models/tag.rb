class Tag < ApplicationRecord
  normalizes :name, with: ->(name) { name.strip.downcase }

  validates :name, presence: true, uniqueness: true
end
