# Temporary on-disk staging for async uploads: the HTTP request writes the raw
# bytes here, the background job reads and deletes them.
class StagingStore
  def self.root
    Rails.root.join("storage", "staging").tap { |p| FileUtils.mkdir_p(p) }
  end

  def self.write(id, io)
    io.rewind if io.respond_to?(:rewind)
    path = root.join(id.to_s)
    File.open(path, "wb") { |f| IO.copy_stream(io, f) }
    path
  end

  def self.path(id)
    root.join(id.to_s)
  end

  def self.delete(id)
    FileUtils.rm_f(path(id))
  end
end
