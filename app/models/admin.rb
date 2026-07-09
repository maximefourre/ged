class Admin < ApplicationRecord
  devise :database_authenticatable

  validates :username, presence: true, uniqueness: { case_sensitive: false }
end
