# Facade over Solid Queue's native pause for the dedicated uploads queue
# (StoreDocumentJob). Pausing stops workers from claiming new store jobs; a
# job already running finishes and its callback (on :default) still goes out.
module UploadQueue
  NAME = "uploads"

  def self.pause!
    SolidQueue::Queue.find_by_name(NAME).pause
  end

  def self.resume!
    SolidQueue::Queue.find_by_name(NAME).resume
  end

  def self.paused?
    SolidQueue::Pause.exists?(queue_name: NAME)
  end
end
