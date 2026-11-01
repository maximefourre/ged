CREATE TABLE public.admins (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    encrypted_password character varying DEFAULT ''::character varying NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    username character varying NOT NULL
);
CREATE SEQUENCE public.admins_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.admins_id_seq OWNED BY public.admins.id;
CREATE TABLE public.app_clients (
    id bigint NOT NULL,
    client_identifier character varying NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    description character varying,
    updated_at timestamp(6) without time zone NOT NULL
);
CREATE SEQUENCE public.app_clients_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.app_clients_id_seq OWNED BY public.app_clients.id;
CREATE TABLE public.client_department_accesses (
    id bigint NOT NULL,
    app_client_id bigint NOT NULL,
    can_read boolean DEFAULT true NOT NULL,
    can_write boolean DEFAULT true NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    department_id bigint NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    CONSTRAINT at_least_one_permission CHECK ((can_read OR can_write))
);
CREATE SEQUENCE public.client_department_accesses_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.client_department_accesses_id_seq OWNED BY public.client_department_accesses.id;
CREATE TABLE public.departments (
    id bigint NOT NULL,
    created_at timestamp(6) without time zone NOT NULL,
    deleted_at timestamp(6) without time zone,
    name character varying NOT NULL,
    parent_id bigint,
    updated_at timestamp(6) without time zone NOT NULL
);
CREATE SEQUENCE public.departments_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE public.departments_id_seq OWNED BY public.departments.id;
CREATE TABLE public.documents (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    byte_size bigint,
    content_type character varying,
    created_at timestamp(6) without time zone NOT NULL,
    deleted_at timestamp(6) without time zone,
    department_id bigint NOT NULL,
    filename character varying NOT NULL,
    purge_at timestamp(6) without time zone,
    stored_async boolean DEFAULT false NOT NULL,
    tags jsonb DEFAULT '{}'::jsonb NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    uploaded_at timestamp(6) without time zone NOT NULL,
    uploaded_by character varying NOT NULL
);
CREATE TABLE public.upload_requests (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    callback_url character varying NOT NULL,
    content_type character varying,
    created_at timestamp(6) without time zone NOT NULL,
    department_id bigint NOT NULL,
    document_id uuid,
    error_message character varying,
    filename character varying NOT NULL,
    status character varying DEFAULT 'pending'::character varying NOT NULL,
    tags jsonb DEFAULT '{}'::jsonb NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    uploaded_by character varying NOT NULL
);
ALTER TABLE ONLY public.admins ALTER COLUMN id SET DEFAULT nextval('public.admins_id_seq'::regclass);
ALTER TABLE ONLY public.app_clients ALTER COLUMN id SET DEFAULT nextval('public.app_clients_id_seq'::regclass);
ALTER TABLE ONLY public.client_department_accesses ALTER COLUMN id SET DEFAULT nextval('public.client_department_accesses_id_seq'::regclass);
ALTER TABLE ONLY public.departments ALTER COLUMN id SET DEFAULT nextval('public.departments_id_seq'::regclass);
ALTER TABLE ONLY public.admins
    ADD CONSTRAINT admins_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.app_clients
    ADD CONSTRAINT app_clients_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.client_department_accesses
    ADD CONSTRAINT client_department_accesses_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.departments
    ADD CONSTRAINT departments_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.documents
    ADD CONSTRAINT documents_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.upload_requests
    ADD CONSTRAINT upload_requests_pkey PRIMARY KEY (id);
CREATE UNIQUE INDEX index_admins_on_username ON public.admins USING btree (username);
CREATE UNIQUE INDEX index_app_clients_on_client_identifier ON public.app_clients USING btree (client_identifier);
CREATE INDEX index_client_department_accesses_on_app_client_id ON public.client_department_accesses USING btree (app_client_id);
CREATE INDEX index_client_department_accesses_on_department_id ON public.client_department_accesses USING btree (department_id);
CREATE UNIQUE INDEX index_client_department_accesses_uniqueness ON public.client_department_accesses USING btree (app_client_id, department_id);
CREATE INDEX index_departments_on_deleted_at ON public.departments USING btree (deleted_at);
CREATE UNIQUE INDEX index_departments_on_name_at_root ON public.departments USING btree (name) WHERE (parent_id IS NULL);
CREATE INDEX index_departments_on_parent_id ON public.departments USING btree (parent_id);
CREATE UNIQUE INDEX index_departments_on_parent_id_and_name ON public.departments USING btree (parent_id, name);
CREATE INDEX index_documents_async ON public.documents USING btree (stored_async) WHERE stored_async;
CREATE INDEX index_documents_on_created_at ON public.documents USING btree (created_at);
CREATE INDEX index_documents_on_department_id ON public.documents USING btree (department_id);
CREATE INDEX index_documents_on_purge_at ON public.documents USING btree (purge_at);
CREATE INDEX index_documents_on_tags ON public.documents USING gin (tags);
CREATE INDEX index_documents_on_updated_at ON public.documents USING btree (updated_at);
CREATE INDEX index_documents_on_uploaded_at ON public.documents USING btree (uploaded_at);
CREATE INDEX index_documents_on_uploaded_by ON public.documents USING btree (uploaded_by);
CREATE INDEX index_upload_requests_on_department_id ON public.upload_requests USING btree (department_id);
ALTER TABLE ONLY public.upload_requests
    ADD CONSTRAINT fk_rails_293ac39269 FOREIGN KEY (department_id) REFERENCES public.departments(id);
ALTER TABLE ONLY public.client_department_accesses
    ADD CONSTRAINT fk_rails_305a9e95a7 FOREIGN KEY (department_id) REFERENCES public.departments(id);
ALTER TABLE ONLY public.client_department_accesses
    ADD CONSTRAINT fk_rails_31ad968ef4 FOREIGN KEY (app_client_id) REFERENCES public.app_clients(id);
ALTER TABLE ONLY public.upload_requests
    ADD CONSTRAINT fk_rails_599ba850b4 FOREIGN KEY (document_id) REFERENCES public.documents(id) ON DELETE SET NULL;
ALTER TABLE ONLY public.documents
    ADD CONSTRAINT fk_rails_710cf0c17d FOREIGN KEY (department_id) REFERENCES public.departments(id);
ALTER TABLE ONLY public.departments
    ADD CONSTRAINT fk_rails_8e1e5764fc FOREIGN KEY (parent_id) REFERENCES public.departments(id);
