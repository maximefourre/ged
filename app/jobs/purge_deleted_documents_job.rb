# Hard-deletes documents whose soft-delete retention window has elapsed
# (purge_at in the past): destroy! removes the row and fires Active Storage's
# default async purge for the attached file/blob. Then hard-deletes soft-deleted
# leaf departments that have no referencing rows. Scheduled via Solid Queue
# (config/recurring.yml).
class PurgeDeletedDocumentsJob < ApplicationJob
  queue_as :default

  def perform
    Document.purgeable.find_each(&:destroy!)
    purge_orphaned_departments
  end

  private

  def purge_orphaned_departments
    Department.deleted.find_each do |department|
      next if Document.with_deleted.where(department_id: department.id).exists?
      next if department.upload_requests.exists?
      next if Department.with_deleted.where(parent_id: department.id).exists?
      department.destroy!
    rescue => e
      Rails.logger.error("PurgeDeletedDocumentsJob: failed to purge department #{department.id}: #{e.message}")
    end
  end
end
