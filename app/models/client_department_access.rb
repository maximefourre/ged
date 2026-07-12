class ClientDepartmentAccess < ApplicationRecord
  belongs_to :app_client
  belongs_to :department

  # "No access" is the absence of a row, not a row with both flags off
  # (mirrors the at_least_one_permission DB check constraint).
  validate :at_least_one_permission

  private

  def at_least_one_permission
    return if can_read? || can_write?

    errors.add(:base, "must grant at least read or write access")
  end
end
