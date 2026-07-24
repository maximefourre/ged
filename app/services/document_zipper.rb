# Streams a set of documents' files as ZIP entries for the admin bulk
# download. Only yields the entry name and the Active Storage blob — the
# caller (Admin::DocumentsController, via zip_kit) is responsible for
# actually streaming bytes out of the blob into the ZIP sink, so this
# service never has to buffer a whole file in memory.
class DocumentZipper
  def initialize(documents)
    @documents = documents
  end

  # Yields [entry_name, blob] for every document whose file is actually
  # attached. doc.filename is not unique across documents, so entries are
  # prefixed with the document id — that alone guarantees uniqueness, no
  # extra collision suffixing needed. Documents with no attached file (or a
  # blob that has gone missing) are skipped and logged rather than aborting
  # the whole archive.
  def each_entry
    @documents.each do |doc|
      unless doc.file.attached?
        Rails.logger.warn("DocumentZipper: skipping document #{doc.id} — no file attached")
        next
      end

      yield "#{doc.id}-#{doc.filename}", doc.file.blob
    rescue ActiveStorage::FileNotFoundError
      Rails.logger.warn("DocumentZipper: skipping document #{doc.id} — blob file missing")
    end
  end
end
