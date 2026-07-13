# Free/total space of the filesystem holding `path`, via POSIX `df -Pk`
# (portable columns, 1024-byte blocks) — Ruby has no statvfs in the stdlib
# and a native gem is overkill for one number. Returns nil on any failure.
class DiskSpace
  Result = Data.define(:total_kb, :available_kb) do
    def used_fraction
      total_kb.zero? ? 0.0 : 1.0 - (available_kb.to_f / total_kb)
    end
  end

  def self.for(path)
    out = IO.popen([ "df", "-Pk", path.to_s ], err: File::NULL, &:read)
    return nil unless $?.success? && (line = out.lines[1])
    _fs, total, _used, available, = line.split
    Result.new(total_kb: total.to_i, available_kb: available.to_i)
  rescue SystemCallError
    nil
  end
end
