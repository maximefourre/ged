# Single home for "build a Document, attach the bytes, save": the sync API
# upload, the admin UI upload and the async StoreDocumentJob all create
# documents through here. Raises ActiveRecord::RecordInvalid on validation
# failure.
class DocumentCreator
  def self.call(department_id:, io:, filename:, content_type:, byte_size:, uploaded_by:,
                tags: {}, uploaded_at: nil, stored_async: false)
    document = Document.new(
      department_id: department_id,
      filename: filename,
      content_type: content_type,
      byte_size: byte_size,
      uploaded_by: uploaded_by,
      tags: tags,
      uploaded_at: uploaded_at,
      stored_async: stored_async
    )
    document.file.attach(io: io, filename: filename, content_type: content_type)
    document.save!
    document
  end
end
