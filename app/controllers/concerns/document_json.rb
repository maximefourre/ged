# Shared API serialization for documents (index/show/create and tag update).
module DocumentJson
  private

  def document_json(document)
    {
      id: document.id,
      filename: document.filename,
      content_type: document.content_type,
      byte_size: document.byte_size,
      department_id: document.department_id,
      uploaded_by: document.uploaded_by,
      tags: document.tags,
      created_at: document.created_at,
      metadata: document.metadata_hash
    }
  end
end
