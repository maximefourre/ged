# Sends a document's file without loading the whole blob in memory (async
# upload exists because files can be big — downloads have the same problem).
# The disk service (this app's storage) exposes the file path, so send_file
# streams straight from disk; any other service falls back to send_data.
module DocumentStreaming
  private

  def send_document_file(document)
    blob = document.file.blob
    options = { filename: document.filename,
                type: document.content_type || "application/octet-stream" }
    if blob.service.respond_to?(:path_for)
      send_file blob.service.path_for(blob.key), **options
    else
      send_data blob.download, **options
    end
  end
end
