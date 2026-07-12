# This file is auto-generated from the current state of the database. Instead
# of editing this file, please use the migrations feature of Active Record to
# incrementally modify your database, and then regenerate this schema definition.
#
# This file is the source Rails uses to define your schema when running `bin/rails
# db:schema:load`. When creating a new database, `bin/rails db:schema:load` tends to
# be faster and is potentially less error prone than running all of your
# migrations from scratch. Old migrations may fail to apply correctly if those
# migrations use external dependencies or application code.
#
# It's strongly recommended that you check this file into your version control system.

ActiveRecord::Schema[8.1].define(version: 2026_07_12_120000) do
  # These are extensions that must be enabled in order to support this database
  enable_extension "pg_catalog.plpgsql"
  enable_extension "pgcrypto"

  create_table "active_storage_attachments", force: :cascade do |t|
    t.bigint "blob_id", null: false
    t.datetime "created_at", null: false
    t.string "name", null: false
    t.uuid "record_id", null: false
    t.string "record_type", null: false
    t.index ["blob_id"], name: "index_active_storage_attachments_on_blob_id"
    t.index ["record_type", "record_id", "name", "blob_id"], name: "index_active_storage_attachments_uniqueness", unique: true
  end

  create_table "active_storage_blobs", force: :cascade do |t|
    t.bigint "byte_size", null: false
    t.string "checksum"
    t.string "content_type"
    t.datetime "created_at", null: false
    t.string "filename", null: false
    t.string "key", null: false
    t.text "metadata"
    t.string "service_name", null: false
    t.index ["key"], name: "index_active_storage_blobs_on_key", unique: true
  end

  create_table "active_storage_variant_records", force: :cascade do |t|
    t.bigint "blob_id", null: false
    t.string "variation_digest", null: false
    t.index ["blob_id", "variation_digest"], name: "index_active_storage_variant_records_uniqueness", unique: true
  end

  create_table "admins", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.string "encrypted_password", default: "", null: false
    t.datetime "updated_at", null: false
    t.string "username", null: false
    t.index ["username"], name: "index_admins_on_username", unique: true
  end

  create_table "app_clients", force: :cascade do |t|
    t.string "client_identifier", null: false
    t.datetime "created_at", null: false
    t.string "description"
    t.datetime "updated_at", null: false
    t.index ["client_identifier"], name: "index_app_clients_on_client_identifier", unique: true
  end

  create_table "client_department_accesses", force: :cascade do |t|
    t.bigint "app_client_id", null: false
    t.boolean "can_read", default: true, null: false
    t.boolean "can_write", default: true, null: false
    t.datetime "created_at", null: false
    t.bigint "department_id", null: false
    t.datetime "updated_at", null: false
    t.index ["app_client_id", "department_id"], name: "index_client_department_accesses_uniqueness", unique: true
    t.index ["app_client_id"], name: "index_client_department_accesses_on_app_client_id"
    t.index ["department_id"], name: "index_client_department_accesses_on_department_id"
    t.check_constraint "can_read OR can_write", name: "at_least_one_permission"
  end

  create_table "departments", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.string "name", null: false
    t.bigint "parent_id"
    t.datetime "updated_at", null: false
    t.index ["name"], name: "index_departments_on_name_at_root", unique: true, where: "(parent_id IS NULL)"
    t.index ["parent_id", "name"], name: "index_departments_on_parent_id_and_name", unique: true
    t.index ["parent_id"], name: "index_departments_on_parent_id"
  end

  create_table "documents", id: :uuid, default: -> { "gen_random_uuid()" }, force: :cascade do |t|
    t.bigint "byte_size"
    t.string "content_type"
    t.datetime "created_at", null: false
    t.datetime "deleted_at"
    t.bigint "department_id", null: false
    t.string "filename", null: false
    t.datetime "purge_at"
    t.boolean "stored_async", default: false, null: false
    t.jsonb "tags", default: {}, null: false
    t.datetime "updated_at", null: false
    t.datetime "uploaded_at", null: false
    t.string "uploaded_by", null: false
    t.index ["created_at"], name: "index_documents_on_created_at"
    t.index ["department_id"], name: "index_documents_on_department_id"
    t.index ["purge_at"], name: "index_documents_on_purge_at"
    t.index ["stored_async"], name: "index_documents_async", where: "stored_async"
    t.index ["tags"], name: "index_documents_on_tags", using: :gin
    t.index ["updated_at"], name: "index_documents_on_updated_at"
    t.index ["uploaded_at"], name: "index_documents_on_uploaded_at"
    t.index ["uploaded_by"], name: "index_documents_on_uploaded_by"
  end

  create_table "upload_requests", id: :uuid, default: -> { "gen_random_uuid()" }, force: :cascade do |t|
    t.string "callback_url", null: false
    t.string "content_type"
    t.datetime "created_at", null: false
    t.bigint "department_id", null: false
    t.uuid "document_id"
    t.string "error_message"
    t.string "filename", null: false
    t.string "status", default: "pending", null: false
    t.jsonb "tags", default: {}, null: false
    t.datetime "updated_at", null: false
    t.string "uploaded_by", null: false
    t.index ["department_id"], name: "index_upload_requests_on_department_id"
  end

  add_foreign_key "active_storage_attachments", "active_storage_blobs", column: "blob_id"
  add_foreign_key "active_storage_variant_records", "active_storage_blobs", column: "blob_id"
  add_foreign_key "client_department_accesses", "app_clients"
  add_foreign_key "client_department_accesses", "departments"
  add_foreign_key "departments", "departments", column: "parent_id"
  add_foreign_key "documents", "departments"
  add_foreign_key "upload_requests", "departments"
  add_foreign_key "upload_requests", "documents", on_delete: :nullify
end
