-- Business tables of GED, identical to Rails' db/schema.rb (columns in the
-- order Rails creates them, types, defaults, constraints, index and foreign
-- key names). `parity_schema` compares `pg_dump --schema-only` of these
-- tables on the Rails and the Rust side: keep them byte-for-byte in step.
--
-- Not here on purpose: Active Storage tables (replaced by the storage layer)
-- and Solid Queue/Cache tables (replaced by the job queue); both arrive with
-- the tasks that own them.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE admins (
    id bigserial,
    created_at timestamp(6) NOT NULL,
    encrypted_password varchar NOT NULL DEFAULT '',
    updated_at timestamp(6) NOT NULL,
    username varchar NOT NULL,
    CONSTRAINT admins_pkey PRIMARY KEY (id)
);
CREATE UNIQUE INDEX index_admins_on_username ON admins (username);

CREATE TABLE app_clients (
    id bigserial,
    client_identifier varchar NOT NULL,
    created_at timestamp(6) NOT NULL,
    description varchar,
    updated_at timestamp(6) NOT NULL,
    CONSTRAINT app_clients_pkey PRIMARY KEY (id)
);
CREATE UNIQUE INDEX index_app_clients_on_client_identifier ON app_clients (client_identifier);

CREATE TABLE departments (
    id bigserial,
    created_at timestamp(6) NOT NULL,
    deleted_at timestamp(6),
    name varchar NOT NULL,
    parent_id bigint,
    updated_at timestamp(6) NOT NULL,
    CONSTRAINT departments_pkey PRIMARY KEY (id)
);
CREATE INDEX index_departments_on_deleted_at ON departments (deleted_at);
CREATE UNIQUE INDEX index_departments_on_name_at_root ON departments (name) WHERE (parent_id IS NULL);
CREATE INDEX index_departments_on_parent_id ON departments (parent_id);
CREATE UNIQUE INDEX index_departments_on_parent_id_and_name ON departments (parent_id, name);
ALTER TABLE departments
    ADD CONSTRAINT fk_rails_8e1e5764fc FOREIGN KEY (parent_id) REFERENCES departments (id);

CREATE TABLE client_department_accesses (
    id bigserial,
    app_client_id bigint NOT NULL,
    can_read boolean NOT NULL DEFAULT true,
    can_write boolean NOT NULL DEFAULT true,
    created_at timestamp(6) NOT NULL,
    department_id bigint NOT NULL,
    updated_at timestamp(6) NOT NULL,
    CONSTRAINT client_department_accesses_pkey PRIMARY KEY (id),
    CONSTRAINT at_least_one_permission CHECK (can_read OR can_write)
);
CREATE INDEX index_client_department_accesses_on_app_client_id ON client_department_accesses (app_client_id);
CREATE INDEX index_client_department_accesses_on_department_id ON client_department_accesses (department_id);
CREATE UNIQUE INDEX index_client_department_accesses_uniqueness ON client_department_accesses (app_client_id, department_id);
ALTER TABLE client_department_accesses
    ADD CONSTRAINT fk_rails_31ad968ef4 FOREIGN KEY (app_client_id) REFERENCES app_clients (id);
ALTER TABLE client_department_accesses
    ADD CONSTRAINT fk_rails_305a9e95a7 FOREIGN KEY (department_id) REFERENCES departments (id);

CREATE TABLE documents (
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    byte_size bigint,
    content_type varchar,
    created_at timestamp(6) NOT NULL,
    deleted_at timestamp(6),
    department_id bigint NOT NULL,
    filename varchar NOT NULL,
    purge_at timestamp(6),
    stored_async boolean NOT NULL DEFAULT false,
    tags jsonb NOT NULL DEFAULT '{}',
    updated_at timestamp(6) NOT NULL,
    uploaded_at timestamp(6) NOT NULL,
    uploaded_by varchar NOT NULL,
    CONSTRAINT documents_pkey PRIMARY KEY (id)
);
CREATE INDEX index_documents_async ON documents (stored_async) WHERE stored_async;
CREATE INDEX index_documents_on_created_at ON documents (created_at);
CREATE INDEX index_documents_on_department_id ON documents (department_id);
CREATE INDEX index_documents_on_purge_at ON documents (purge_at);
CREATE INDEX index_documents_on_tags ON documents USING gin (tags);
CREATE INDEX index_documents_on_updated_at ON documents (updated_at);
CREATE INDEX index_documents_on_uploaded_at ON documents (uploaded_at);
CREATE INDEX index_documents_on_uploaded_by ON documents (uploaded_by);
ALTER TABLE documents
    ADD CONSTRAINT fk_rails_710cf0c17d FOREIGN KEY (department_id) REFERENCES departments (id);

CREATE TABLE upload_requests (
    id uuid NOT NULL DEFAULT gen_random_uuid(),
    callback_url varchar NOT NULL,
    content_type varchar,
    created_at timestamp(6) NOT NULL,
    department_id bigint NOT NULL,
    document_id uuid,
    error_message varchar,
    filename varchar NOT NULL,
    status varchar NOT NULL DEFAULT 'pending',
    tags jsonb NOT NULL DEFAULT '{}',
    updated_at timestamp(6) NOT NULL,
    uploaded_by varchar NOT NULL,
    CONSTRAINT upload_requests_pkey PRIMARY KEY (id)
);
CREATE INDEX index_upload_requests_on_department_id ON upload_requests (department_id);
ALTER TABLE upload_requests
    ADD CONSTRAINT fk_rails_293ac39269 FOREIGN KEY (department_id) REFERENCES departments (id);
ALTER TABLE upload_requests
    ADD CONSTRAINT fk_rails_599ba850b4 FOREIGN KEY (document_id) REFERENCES documents (id) ON DELETE SET NULL;
