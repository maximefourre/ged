# Hard-deletes documents whose soft-delete retention window has elapsed
# (purge_at in the past): destroy! removes the row and fires Active Storage's
# default async purge for the attached file/blob. Scheduled via Solid Queue
# (config/recurring.yml).
class PurgeDeletedDocumentsJob < ApplicationJob
  queue_as :default

  def perform
    Document.purgeable.find_each(&:destroy!)
  end
end
