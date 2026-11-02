-- T6: the stored file behind a document (replaces Active Storage's blobs and
-- attachments). Not a Rails business table: parity_schema does not compare it.
-- `key` is the 32-hex name of the file on disk (see storage/disk.rs); one file
-- per document, removed with its document row.

CREATE TABLE stored_files (
    key varchar NOT NULL,
    document_id uuid NOT NULL,
    filename varchar NOT NULL,
    content_type varchar NOT NULL,
    byte_size bigint NOT NULL,
    checksum varchar NOT NULL,
    created_at timestamp(6) NOT NULL,
    CONSTRAINT stored_files_pkey PRIMARY KEY (key),
    CONSTRAINT stored_files_key_format CHECK (key ~ '^[0-9a-f]{32}$'),
    CONSTRAINT fk_stored_files_document FOREIGN KEY (document_id)
        REFERENCES documents (id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX index_stored_files_on_document_id ON stored_files (document_id);
