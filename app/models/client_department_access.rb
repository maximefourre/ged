class ClientDepartmentAccess < ApplicationRecord
  belongs_to :app_client
  belongs_to :department
end
