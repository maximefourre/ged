# KV Tags + Upload Queue Pause Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat tag registry with a jsonb key→value tag map passable at upload and returned in the async callback, and add admin pause/resume of a dedicated `uploads` Solid Queue queue.

**Architecture:** One corrective migration moves tags from `tags`/`document_tags` tables to a `documents.tags` jsonb map (GIN-indexed) plus `upload_requests.tags`; a shared `KvTaggable` concern normalizes and validates the map. `StoreDocumentJob` moves to a dedicated `uploads` queue that admins pause/resume via Solid Queue's native pause (callbacks stay on `default` so they always deliver). Dev/test gain a queue database so the pause is testable locally.

**Tech Stack:** Rails 8.1 (API + ERB admin), PostgreSQL jsonb, Solid Queue 1.4, Minitest + WebMock.

**Spec:** `docs/superpowers/specs/2026-07-11-kv-tags-and-upload-queue-pause-design.md`

## Global Constraints

- `bin/rubocop` (rails-omakase) must pass before every commit.
- Run tests with `bin/rails test` (system tests excluded; `bin/ci` runs everything).
- Tests are Minitest integration-style; **no fixture records** — create records inline in `setup` (see any existing test).
- API tests use `JwtTestHelpers`: `stub_jwks!` in setup, `bearer_header("erp-app")` for auth headers. `Auth::JwksCache.reset!` runs automatically.
- WebMock quirk: `assert_requested` rejects the `(stub) { block }` form — use `assert_requested(:post, url) { |req| … }`.
- Migrations inherit `ActiveRecord::Migration[8.1]`.
- API error contract: 401 invalid/missing token, 403 forbidden, 404 not found, 400 missing/malformed param, 422 validation.
- Tag map rules (spec): single-level map; keys strip+downcase, non-empty, ≤ 100 chars; values stripped strings (blank → `null`) or `null`, ≤ 500 chars; ≤ 50 entries.
- Commit messages follow the repo style (`feat:`/`fix:`/`docs:` + one line) and end with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

### Task 1: Cutover — replace the tag registry with the jsonb map

Everything referencing `Tag`/`DocumentTag` switches in one task so the suite is green at the end: migration + backfill, `KvTaggable` concern, API serialization/filters, `PUT /documents/:id/tags`, registry endpoint removal, admin views. Tags-at-upload comes in Task 2.

**Files:**
- Create: `db/migrate/20260711000001_replace_tag_registry_with_jsonb_tags.rb`
- Create: `app/models/concerns/kv_taggable.rb`
- Create: `app/controllers/concerns/document_json.rb`
- Create: `app/helpers/tags_helper.rb`
- Create: `test/models/tags_map_validation_test.rb`
- Create: `test/controllers/api/v1/document_tags_test.rb`
- Modify: `app/models/document.rb`, `app/models/upload_request.rb`
- Modify: `app/controllers/api/v1/base_controller.rb` (add `tags_param`)
- Modify: `app/controllers/api/v1/documents_controller.rb` (filters, serialization)
- Rewrite: `app/controllers/api/v1/document_tags_controller.rb` (`update` replaces `create`)
- Modify: `app/controllers/admin/documents_controller.rb` (index filter, update via textarea)
- Modify: `app/views/admin/documents/index.html.erb`, `app/views/admin/documents/edit.html.erb`
- Modify: `app/views/layouts/admin.html.erb` (drop Tags nav link)
- Modify: `config/routes.rb`
- Modify: `test/controllers/api/v1/documents_test.rb`, `test/controllers/admin/admin_ui_test.rb`
- Delete: `app/models/tag.rb`, `app/models/document_tag.rb`, `app/controllers/api/v1/tags_controller.rb`, `app/controllers/admin/tags_controller.rb`, `app/views/admin/tags/`, `test/models/tag_test.rb`, `test/controllers/api/v1/tags_test.rb`

**Interfaces:**
- Consumes: existing `Document`, `UploadRequest`, `DocumentCreator`, `Api::V1::BaseController` helpers (`scalar_param`, `BadParameter`).
- Produces: `Document#tags` / `UploadRequest#tags` → `Hash` (jsonb map, normalized); `KvTaggable` concern; `tags_param` (Api::V1::BaseController private, returns `Hash`/passthrough); `DocumentJson#document_json(document)` → hash with `tags:` map; routes `PUT /api/v1/documents/:document_id/tags`; helpers `format_tags(map)` and `tags_text(map)`.

- [ ] **Step 1: Write the migration**

`db/migrate/20260711000001_replace_tag_registry_with_jsonb_tags.rb`:

```ruby
class ReplaceTagRegistryWithJsonbTags < ActiveRecord::Migration[8.1]
  def up
    add_column :documents, :tags, :jsonb, null: false, default: {}
    add_column :upload_requests, :tags, :jsonb, null: false, default: {}
    add_index :documents, :tags, using: :gin

    # Each flat tag name becomes a key with a null value.
    execute <<~SQL
      UPDATE documents
      SET tags = agg.map
      FROM (
        SELECT document_tags.document_id, jsonb_object_agg(tags.name, NULL::text) AS map
        FROM document_tags
        JOIN tags ON tags.id = document_tags.tag_id
        GROUP BY document_tags.document_id
      ) agg
      WHERE documents.id = agg.document_id
    SQL

    drop_table :document_tags
    drop_table :tags
  end

  def down
    raise ActiveRecord::IrreversibleMigration
  end
end
```

- [ ] **Step 2: Run the migration**

Run: `bin/rails db:migrate && bin/rails db:test:prepare`
Expected: migrates cleanly; `db/schema.rb` shows `t.jsonb "tags", default: {}, null: false` on both tables, the GIN index, and no `tags`/`document_tags` tables.

- [ ] **Step 3: Write the failing model tests**

`test/models/tags_map_validation_test.rb`:

```ruby
require "test_helper"

class TagsMapValidationTest < ActiveSupport::TestCase
  setup do
    @finance = Department.create!(name: "Finance")
  end

  def build_document(tags)
    Document.new(department: @finance, filename: "r.pdf", uploaded_by: "seed", tags: tags)
  end

  test "accepts a flat map of string keys with string or null values" do
    assert build_document({ "type" => "facture", "urgent" => nil }).valid?
    assert build_document({}).valid?
  end

  test "normalizes keys (strip+downcase) and values (strip, blank becomes null)" do
    doc = build_document({ "  Type " => "  Facture ", "urgent" => "   " })
    assert_equal({ "type" => "Facture", "urgent" => nil }, doc.tags)
  end

  test "symbol keys are stringified" do
    assert_equal({ "type" => "facture" }, build_document({ type: "facture" }).tags)
  end

  test "rejects values that are not a map" do
    [ nil, "flat", 42, [ "a", "b" ] ].each do |bad|
      doc = build_document(bad)
      assert_not doc.valid?, "expected #{bad.inspect} to be invalid"
      assert doc.errors[:tags].any?
    end
  end

  test "rejects nested or non-string values" do
    [ { "k" => { "nested" => "x" } }, { "k" => [ "x" ] }, { "k" => 42 }, { "k" => true } ].each do |bad|
      assert_not build_document(bad).valid?, "expected #{bad.inspect} to be invalid"
    end
  end

  test "rejects blank and overlong keys, and overlong values" do
    assert_not build_document({ "  " => "x" }).valid?
    assert_not build_document({ "k" * 101 => "x" }).valid?
    assert_not build_document({ "k" => "v" * 501 }).valid?
  end

  test "rejects more than 50 entries" do
    doc = build_document(51.times.to_h { |i| [ "key-#{i}", nil ] })
    assert_not doc.valid?
  end

  test "upload requests validate tags the same way" do
    request = UploadRequest.new(department: @finance, filename: "r.pdf",
                                callback_url: "https://caller.test/cb",
                                uploaded_by: "erp-app", tags: "flat")
    assert_not request.valid?
    request.tags = { "Type" => " facture " }
    assert request.valid?
    assert_equal({ "type" => "facture" }, request.tags)
  end
end
```

- [ ] **Step 4: Run the model tests to verify they fail**

Run: `bin/rails test test/models/tags_map_validation_test.rb`
Expected: errors — `Document` still has the `tags` association (or unknown attribute), `KvTaggable` missing.

- [ ] **Step 5: Implement the concern and switch the models**

`app/models/concerns/kv_taggable.rb`:

```ruby
# Normalizes and validates a single-level key→value tag map stored in a
# jsonb `tags` column. Keys are strip+downcased; values are stripped strings
# (blank collapses to null) or null. Malformed input is passed through
# untouched so validation rejects it instead of masking it.
module KvTaggable
  extend ActiveSupport::Concern

  MAX_ENTRIES = 50
  MAX_KEY_LENGTH = 100
  MAX_VALUE_LENGTH = 500

  included do
    validate :tags_must_be_a_valid_map
  end

  def tags=(value)
    super(normalize_tags(value))
  end

  private

  def normalize_tags(value)
    return value unless value.is_a?(Hash)
    value.to_h do |key, val|
      key = key.to_s if key.is_a?(Symbol)
      [
        key.is_a?(String) ? key.strip.downcase : key,
        val.is_a?(String) ? val.strip.presence : val
      ]
    end
  end

  def tags_must_be_a_valid_map
    return errors.add(:tags, "must be a map of key/value pairs") unless tags.is_a?(Hash)

    errors.add(:tags, "cannot have more than #{MAX_ENTRIES} entries") if tags.size > MAX_ENTRIES
    unless tags.keys.all? { |k| k.is_a?(String) && k.present? && k.length <= MAX_KEY_LENGTH }
      errors.add(:tags, "keys must be non-empty strings of at most #{MAX_KEY_LENGTH} characters")
    end
    unless tags.values.all? { |v| v.nil? || (v.is_a?(String) && v.length <= MAX_VALUE_LENGTH) }
      errors.add(:tags, "values must be strings of at most #{MAX_VALUE_LENGTH} characters, or null")
    end
  end
end
```

`app/models/document.rb`:

```ruby
class Document < ApplicationRecord
  include KvTaggable

  belongs_to :department
  has_one_attached :file

  validates :filename, :uploaded_by, presence: true
end
```

`app/models/upload_request.rb` — add `include KvTaggable` right under the class line (keep everything else):

```ruby
class UploadRequest < ApplicationRecord
  include KvTaggable

  STATUSES = %w[pending processing completed failed].freeze
  # … rest unchanged
```

Delete `app/models/tag.rb` and `app/models/document_tag.rb`.

- [ ] **Step 6: Run the model tests to verify they pass**

Run: `bin/rails test test/models/tags_map_validation_test.rb`
Expected: PASS (other suites still broken — that's expected until Step 12).

- [ ] **Step 7: Write the failing API tests (PUT tags + filters + serialization)**

Delete `test/controllers/api/v1/tags_test.rb` and `test/models/tag_test.rb`.

Create `test/controllers/api/v1/document_tags_test.rb`:

```ruby
require "test_helper"

class Api::V1::DocumentTagsTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @hr = Department.create!(name: "HR")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance
    @document = create_document(@finance, tags: { "draft" => nil })
  end

  def create_document(department, tags: {})
    doc = Document.new(department: department, filename: "report.pdf",
                       uploaded_by: "erp-app", tags: tags)
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!
    doc
  end

  test "PUT replaces the whole tag map" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("erp-app"),
        params: { tags: { type: "facture", annee: "2026" } }
    assert_response :success
    assert_equal({ "type" => "facture", "annee" => "2026" },
                 JSON.parse(response.body)["tags"])
    assert_equal({ "type" => "facture", "annee" => "2026" }, @document.reload.tags)
  end

  test "PUT without tags clears the map" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("erp-app")
    assert_response :success
    assert_equal({}, @document.reload.tags)
  end

  test "keys and values are normalized" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("erp-app"),
        params: { tags: { "  Type " => "  Facture  " } }
    assert_equal({ "type" => "Facture" }, @document.reload.tags)
  end

  test "rejects a non-map tags param with 422 and leaves tags untouched" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("erp-app"),
        params: { tags: "flat" }
    assert_response :unprocessable_entity
    assert_equal({ "draft" => nil }, @document.reload.tags)
  end

  test "rejects nested values with 422" do
    put "/api/v1/documents/#{@document.id}/tags", headers: bearer_header("erp-app"),
        params: { tags: { type: { nested: "x" } } }
    assert_response :unprocessable_entity
  end

  test "forbidden for a document outside granted departments" do
    other = create_document(@hr)
    put "/api/v1/documents/#{other.id}/tags", headers: bearer_header("erp-app"),
        params: { tags: { type: "facture" } }
    assert_response :forbidden
  end

  test "the tag registry endpoints are gone" do
    assert_raises(ActionController::RoutingError) do
      get "/api/v1/tags", headers: bearer_header("erp-app")
    end
  end
end
```

In `test/controllers/api/v1/documents_test.rb`:

Replace the test `"list returns only accessible documents, with filters"` with:

```ruby
  test "list returns only accessible documents, with filters" do
    doc = create_document(@invoices, "facture-2026.pdf")
    create_document(@hr, "salaries.pdf")
    doc.update!(tags: { "invoice" => "2026" })

    get "/api/v1/documents", headers: bearer_header("erp-app")
    names = JSON.parse(response.body).map { |d| d["filename"] }
    assert_equal [ "facture-2026.pdf" ], names

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { filename: "facture" }
    assert_equal 1, JSON.parse(response.body).size

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "invoice" }
    assert_equal 1, JSON.parse(response.body).size

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "nope" }
    assert_equal 0, JSON.parse(response.body).size
  end
```

Replace the test `"tag filter does not hide the document's other tags"` with:

```ruby
  test "documents carry their tags as a map and tag_value filters on the pair" do
    doc = create_document(@invoices, "facture-2026.pdf")
    doc.update!(tags: { "type" => "facture", "urgent" => nil })

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "type", tag_value: "facture" }
    body = JSON.parse(response.body)
    assert_equal 1, body.size
    assert_equal({ "type" => "facture", "urgent" => nil }, body.first["tags"])

    # filter params are normalized like stored tags
    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: " Type ", tag_value: " facture " }
    assert_equal 1, JSON.parse(response.body).size

    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag: "type", tag_value: "devis" }
    assert_equal 0, JSON.parse(response.body).size
  end

  test "tag_value without tag returns 400" do
    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { tag_value: "facture" }
    assert_response :bad_request
  end
```

Replace the test `"tags and departments lists paginate too"` with (tags half dropped):

```ruby
  test "departments list paginates too" do
    Department.create!(name: "Ops", parent: @finance).tap { |d| @client.departments << d }
    get "/api/v1/departments", headers: bearer_header("erp-app"), params: { limit: 1 }
    assert_equal 1, JSON.parse(response.body).size
  end
```

In the test `"non-string filter params return 400 instead of 500"`, add `tag_value: [ "x" ]` to the hash of bad params:

```ruby
    { filename: [ "x" ], tag: [ "invoice" ], tag_value: [ "x" ],
      department_id: { a: "b" } }.each do |key, value|
```

- [ ] **Step 8: Run the API tests to verify they fail**

Run: `bin/rails test test/controllers/api/v1/document_tags_test.rb test/controllers/api/v1/documents_test.rb`
Expected: FAIL/ERROR (old routes and `Tag` references still in place).

- [ ] **Step 9: Implement the API side**

`app/controllers/concerns/document_json.rb`:

```ruby
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
      created_at: document.created_at
    }
  end
end
```

In `app/controllers/api/v1/base_controller.rb`, add below `require_scalar_param`:

```ruby
      # tags arrives as nested params (tags[type]=facture). Anything that is
      # not a nested hash is passed through so model validation rejects it
      # with 422 instead of a 500 here.
      def tags_param
        value = params[:tags]
        value.respond_to?(:to_unsafe_h) ? value.to_unsafe_h : value
      end
```

In `app/controllers/api/v1/documents_controller.rb`:

- Add `include DocumentJson` next to `include DocumentStreaming` and delete the
  private `document_json` method at the bottom.
- Replace the tag filter block and the `preload` line in `index` (the
  `preload`/`includes` comment goes away — there is no tags association
  anymore):

```ruby
        if (tag = scalar_param(:tag)).present?
          tag = tag.strip.downcase
          if (tag_value = scalar_param(:tag_value)).present?
            scope = scope.where("documents.tags @> ?", { tag => tag_value.strip }.to_json)
          else
            # ? is the jsonb key-exists operator (GIN-served), not a bind marker
            scope = scope.where("documents.tags ? :key", key: tag)
          end
        elsif scalar_param(:tag_value).present?
          raise BadParameter, "tag_value requires tag"
        end
        scope = paginate(scope.order(created_at: :desc, id: :desc))
```

Rewrite `app/controllers/api/v1/document_tags_controller.rb`:

```ruby
module Api
  module V1
    class DocumentTagsController < BaseController
      include DocumentJson

      # Replaces the document's whole tag map (map semantics: the client owns
      # the full map; omitting tags clears it).
      def update
        document = Document.find(params[:document_id])
        authorize_department!(document.department)
        document.update!(tags: tags_param || {})
        render json: document_json(document)
      end
    end
  end
end
```

Delete `app/controllers/api/v1/tags_controller.rb`.

In `config/routes.rb`, replace the API documents/tags block:

```ruby
      resources :documents, only: [ :index, :create, :show, :destroy ] do
        member { get :download }
        resource :tags, only: [ :update ], controller: "document_tags"
      end
      resources :departments, only: [ :index, :create ]
      resources :upload_requests, only: [ :show ]
```

(the standalone `resources :tags, only: [ :index, :create ]` line is removed.)

- [ ] **Step 10: Run the API tests to verify they pass**

Run: `bin/rails test test/controllers/api/v1/`
Expected: PASS.

- [ ] **Step 11: Write the failing admin tests**

In `test/controllers/admin/admin_ui_test.rb`:

Replace the test `"manages a document's tags"` with:

```ruby
  test "manages a document's tags through the key=value textarea" do
    doc = Document.new(department: @finance, filename: "report.pdf", uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!

    get "/admin/documents/#{doc.id}/edit"
    assert_response :success

    patch "/admin/documents/#{doc.id}",
          params: { document: { tags_text: "Type=Facture\nurgent" } }
    assert_redirected_to "/admin/documents"
    assert_equal({ "type" => "Facture", "urgent" => nil }, doc.reload.tags)

    # an empty textarea clears the map
    patch "/admin/documents/#{doc.id}", params: { document: { tags_text: "" } }
    assert_equal({}, doc.reload.tags)

    # an invalid map redirects back to the form with an alert, tags untouched
    patch "/admin/documents/#{doc.id}",
          params: { document: { tags_text: "#{"k" * 101}=x" } }
    assert_redirected_to "/admin/documents/#{doc.id}/edit"
    assert_equal({}, doc.reload.tags)
  end

  test "filters documents by tag key" do
    doc = Document.new(department: @finance, filename: "tagged.pdf", uploaded_by: "seed",
                       tags: { "type" => "facture" })
    doc.file.attach(io: StringIO.new("PDF!"), filename: "tagged.pdf",
                    content_type: "application/pdf")
    doc.save!

    get "/admin/documents", params: { tag: "type" }
    assert_includes response.body, "tagged.pdf"
    assert_includes response.body, "type=facture"

    get "/admin/documents", params: { tag: "absent" }
    assert_not_includes response.body, "tagged.pdf"
  end
```

Delete the test `"creates and deletes a tag"`.

- [ ] **Step 12: Run the admin tests to verify they fail**

Run: `bin/rails test test/controllers/admin/admin_ui_test.rb`
Expected: FAIL/ERROR (`tags_text` unknown, admin tags routes still present).

- [ ] **Step 13: Implement the admin side**

`app/helpers/tags_helper.rb`:

```ruby
module TagsHelper
  # {"type" => "facture", "urgent" => nil} → "type=facture, urgent"
  def format_tags(tags)
    tags.map { |key, value| value.nil? ? key : "#{key}=#{value}" }.join(", ")
  end

  # Textarea form of the map: one key=value pair per line.
  def tags_text(tags)
    tags.map { |key, value| value.nil? ? key : "#{key}=#{value}" }.join("\n")
  end
end
```

In `app/controllers/admin/documents_controller.rb`:

- In `index`, drop `:tags` from `includes` and add the key filter after the
  department filter:

```ruby
    scope = Document.includes(:department).order(created_at: :desc, id: :desc)
    # … filename and department_id filters unchanged …
    if params[:tag].present? && params[:tag].is_a?(String)
      # ? is the jsonb key-exists operator, not a bind marker
      scope = scope.where("documents.tags ? :key", key: params[:tag].strip.downcase)
    end
```

- Replace `update` and add the parser under `private`:

```ruby
  def update
    document = Document.find(params[:id])
    document.update!(tags: parse_tags_text(params.require(:document)[:tags_text]))
    redirect_to admin_documents_path, notice: "Tags updated."
  rescue ActiveRecord::RecordInvalid => e
    redirect_to edit_admin_document_path(document),
                alert: e.record.errors.full_messages.join(", ")
  end

  private

  # One "key=value" pair per line; a line without "=" is a key with no value.
  def parse_tags_text(text)
    text.to_s.each_line.map(&:strip).reject(&:empty?).to_h do |line|
      key, value = line.split("=", 2)
      [ key, value ]
    end
  end
```

`app/views/admin/documents/edit.html.erb`:

```erb
<h1>Tags — <%= @document.filename %></h1>
<%= form_with model: [ :admin, @document ] do |f| %>
  <p>One <code>key=value</code> pair per line; a line without <code>=</code> is a key with no value.</p>
  <div><%= text_area_tag "document[tags_text]", tags_text(@document.tags), rows: 8 %></div>
  <%= f.submit "Save tags" %>
<% end %>
<p><%= link_to "Back to documents", admin_documents_path %></p>
```

In `app/views/admin/documents/index.html.erb`:

- In the filter form, add before the submit button:

```erb
  <%= f.text_field :tag, value: params[:tag], placeholder: "tag key" %>
```

- Change the tags cell from `doc.tags.map(&:name).join(", ")` to:

```erb
      <td><%= format_tags(doc.tags) %></td>
```

In `app/views/layouts/admin.html.erb`, delete the line:

```erb
      <%= link_to "Tags", "/admin/tags" %> |
```

In `config/routes.rb` (admin namespace), delete:

```ruby
    resources :tags, only: [ :index, :create, :destroy ]
```

Delete `app/controllers/admin/tags_controller.rb` and the whole
`app/views/admin/tags/` directory.

- [ ] **Step 14: Run the full suite and rubocop**

Run: `bin/rails test && bin/rubocop`
Expected: everything green, no offenses. If any remaining `Tag`/`document_tags` reference errors appear, run `grep -rn "Tag\b\|document_tags\|tag_ids" app/ test/ config/routes.rb` and fix the stragglers (there must be none left when this step completes).

- [ ] **Step 15: Commit**

```bash
git add -A
git commit -m "feat!: replace the flat tag registry with a jsonb key→value tag map

Documents carry tags as a single-level KV map (GIN-indexed jsonb).
Registry endpoints and admin section removed; PUT /documents/:id/tags
replaces the map; ?tag= filters on key, +tag_value on the exact pair.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Tags at upload (sync, async, admin) + callback/polling payload

**Files:**
- Modify: `app/services/document_creator.rb`
- Modify: `app/controllers/api/v1/documents_controller.rb` (`create`, `create_async`)
- Modify: `app/models/upload_request.rb` (document association)
- Modify: `app/jobs/store_document_job.rb`, `app/jobs/deliver_callback_job.rb`
- Modify: `app/controllers/api/v1/upload_requests_controller.rb`
- Modify: `app/controllers/admin/documents_controller.rb` (`create`), `app/views/admin/documents/new.html.erb`
- Modify: `app/views/admin/upload_requests/index.html.erb`
- Test: `test/controllers/api/v1/documents_test.rb`, `test/controllers/api/v1/async_upload_test.rb`, `test/jobs/store_document_job_test.rb`, `test/jobs/deliver_callback_job_test.rb`, `test/controllers/admin/admin_ui_test.rb`

**Interfaces:**
- Consumes: `tags_param` (Task 1), `KvTaggable` normalization, `DocumentJson`.
- Produces: `DocumentCreator.call(department_id:, io:, filename:, content_type:, byte_size:, uploaded_by:, tags: {})`; `UploadRequest#document` (`belongs_to`, optional); callback payload `{ request_id, status, document_id, error_message, tags }` where `tags` is the document's map on `completed` and `null` on `failed`; `GET /api/v1/upload_requests/:id` mirrors that payload.

- [ ] **Step 1: Write the failing tests**

In `test/controllers/api/v1/documents_test.rb`, add:

```ruby
  test "sync upload accepts tags and returns them in the map" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id, file: upload_file,
                   tags: { Type: " Facture ", urgent: "" } }
    assert_response :created
    body = JSON.parse(response.body)
    assert_equal({ "type" => "Facture", "urgent" => nil }, body["tags"])
    assert_equal({ "type" => "Facture", "urgent" => nil }, Document.find(body["id"]).tags)
  end

  test "sync upload with a non-map tags param returns 422" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id, file: upload_file, tags: "flat" }
    assert_response :unprocessable_entity
    assert_equal 0, Document.count
  end
```

In `test/controllers/api/v1/async_upload_test.rb`, add:

```ruby
  test "async upload applies tags and returns them in the callback and polling" do
    stub_request(:post, @callback_url).to_return(status: 200)

    perform_enqueued_jobs do
      post "/api/v1/documents", headers: bearer_header("erp-app"),
           params: { department_id: @finance.id,
                     file: fixture_file_upload("report.pdf", "application/pdf"),
                     callback_url: @callback_url,
                     tags: { type: "facture", urgent: "" } }
      assert_response :accepted
    end

    request_id = JSON.parse(response.body)["request_id"]
    document = Document.find(UploadRequest.find(request_id).document_id)
    assert_equal({ "type" => "facture", "urgent" => nil }, document.tags)

    assert_requested(:post, @callback_url) do |req|
      payload = JSON.parse(req.body)
      payload["request_id"] == request_id &&
        payload["tags"] == { "type" => "facture", "urgent" => nil }
    end

    get "/api/v1/upload_requests/#{request_id}", headers: bearer_header("erp-app")
    assert_equal({ "type" => "facture", "urgent" => nil },
                 JSON.parse(response.body)["tags"])
  end

  test "async upload with invalid tags is rejected with 422 before the 202" do
    assert_no_enqueued_jobs do
      post "/api/v1/documents", headers: bearer_header("erp-app"),
           params: { department_id: @finance.id,
                     file: fixture_file_upload("report.pdf", "application/pdf"),
                     callback_url: @callback_url,
                     tags: "flat" }
    end
    assert_response :unprocessable_entity
    assert_equal 0, UploadRequest.count
  end

  test "a failed async upload reports null tags in the callback" do
    stub_request(:post, @callback_url).to_return(status: 200)
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id,
                   file: fixture_file_upload("report.pdf", "application/pdf"),
                   callback_url: @callback_url,
                   tags: { type: "facture" } }
    request_id = JSON.parse(response.body)["request_id"]
    StagingStore.delete(request_id) # force the store to fail

    perform_enqueued_jobs

    assert_equal "failed", UploadRequest.find(request_id).status
    assert_requested(:post, @callback_url) do |req|
      payload = JSON.parse(req.body)
      payload["status"] == "failed" && payload.key?("tags") && payload["tags"].nil?
    end
  end
```

In `test/jobs/store_document_job_test.rb`, add (mirror the file's existing
setup pattern for creating an `UploadRequest` + staging file):

```ruby
  test "applies the requested tags to the created document" do
    request = UploadRequest.create!(department: @finance, filename: "report.pdf",
                                    callback_url: "https://caller.test/cb",
                                    uploaded_by: "erp-app",
                                    tags: { "type" => "facture" })
    StagingStore.write(request.id, StringIO.new("PDF!"))

    StoreDocumentJob.perform_now(request.id)

    assert_equal({ "type" => "facture" }, Document.find(request.reload.document_id).tags)
  end
```

(Adjust `@finance` to the department variable that file's `setup` defines.)

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `bin/rails test test/controllers/api/v1/documents_test.rb test/controllers/api/v1/async_upload_test.rb test/jobs/store_document_job_test.rb`
Expected: the new tests FAIL (tags ignored at upload, payloads lack `tags`).

- [ ] **Step 3: Implement**

`app/services/document_creator.rb` — add the `tags:` keyword:

```ruby
class DocumentCreator
  def self.call(department_id:, io:, filename:, content_type:, byte_size:, uploaded_by:, tags: {})
    document = Document.new(
      department_id: department_id,
      filename: filename,
      content_type: content_type,
      byte_size: byte_size,
      uploaded_by: uploaded_by,
      tags: tags
    )
    document.file.attach(io: io, filename: filename, content_type: content_type)
    document.save!
    document
  end
end
```

(keep the existing comment block above the class.)

In `app/controllers/api/v1/documents_controller.rb`:

- sync `create`: add `tags: tags_param || {}` to the `DocumentCreator.call` arguments;
- `create_async`: add `tags: tags_param || {}` to `UploadRequest.create!` —
  validation raises `RecordInvalid` → 422 before any staging write or 202.

In `app/models/upload_request.rb`, add under `belongs_to :department`:

```ruby
  belongs_to :document, optional: true
```

In `app/jobs/store_document_job.rb`, add to the `DocumentCreator.call` arguments:

```ruby
          tags: upload_request.tags,
```

In `app/jobs/deliver_callback_job.rb`, extend the payload:

```ruby
    payload = {
      request_id: upload_request.id,
      status: upload_request.status,
      document_id: upload_request.document_id,
      error_message: upload_request.error_message,
      tags: upload_request.document&.tags
    }
```

In `app/controllers/api/v1/upload_requests_controller.rb`, mirror it:

```ruby
        render json: {
          request_id: upload_request.id,
          status: upload_request.status,
          document_id: upload_request.document_id,
          error_message: upload_request.error_message,
          tags: upload_request.document&.tags
        }
```

- [ ] **Step 4: Run those tests to verify they pass**

Run: `bin/rails test test/controllers/api/v1/ test/jobs/`
Expected: PASS.

- [ ] **Step 5: Admin upload form + upload-requests tags column (with test)**

In `test/controllers/admin/admin_ui_test.rb`, add:

```ruby
  test "uploads a document with tags" do
    post "/admin/documents",
         params: { document: { department_id: @finance.id,
                               file: fixture_file_upload("report.pdf", "application/pdf"),
                               tags_text: "type=facture\nurgent" } }
    assert_redirected_to "/admin/documents"
    doc = Document.order(:created_at).last
    assert_equal({ "type" => "facture", "urgent" => nil }, doc.tags)
  end
```

Run it (`bin/rails test test/controllers/admin/admin_ui_test.rb`) — the new
test FAILS. Then implement:

`app/views/admin/documents/new.html.erb` — add before the submit:

```erb
  <div><%= label_tag "document[tags_text]", "Tags (one key=value per line)" %>
    <%= text_area_tag "document[tags_text]", nil, rows: 5 %></div>
```

`app/controllers/admin/documents_controller.rb#create` — add to the
`DocumentCreator.call` arguments:

```ruby
      tags: parse_tags_text(params.require(:document)[:tags_text]),
```

`app/views/admin/upload_requests/index.html.erb` — add a `<th>Tags</th>`
after `<th>Filename</th>` and the matching cell:

```erb
      <td><%= format_tags(request.tags) %></td>
```

- [ ] **Step 6: Full suite + rubocop**

Run: `bin/rails test && bin/rubocop`
Expected: green, no offenses.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: tags at upload (sync/async/admin), returned by callback and polling

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Solid Queue in development and test

Dev mirrors production (jobs through Solid Queue inside Puma) and test gains
the queue schema, so the pause built in Task 4 is testable. The ActiveJob
**test** adapter stays `:test` — only the Solid Queue tables become available.

**Files:**
- Modify: `config/database.yml` (dev + test become primary/queue multi-db)
- Modify: `config/application.rb` (solid_queue `connects_to` for all envs)
- Modify: `config/environments/production.rb` (remove the now-duplicated `connects_to` line)
- Modify: `config/environments/development.rb` (queue adapter)
- Modify: `bin/dev` (enable the Puma Solid Queue plugin)

**Interfaces:**
- Consumes: existing `config/queue.yml` (has development/test blocks), `db/queue_schema.rb`, `plugin :solid_queue if ENV["SOLID_QUEUE_IN_PUMA"]` in `config/puma.rb`.
- Produces: `SolidQueue::*` models usable in dev/test (backed by `ged_development_queue` / `ged_test_queue`); dev jobs actually processed by `bin/dev`.

- [ ] **Step 1: database.yml — split dev and test into primary + queue**

Replace the `development:` and `test:` sections (keep the surrounding
comments or drop them, rubocop doesn't care) with:

```yaml
development:
  primary:
    <<: *default
    database: ged_development
  queue:
    <<: *default
    database: ged_development_queue
    migrations_paths: db/queue_migrate

test:
  primary:
    <<: *default
    database: ged_test
  queue:
    <<: *default
    database: ged_test_queue
    migrations_paths: db/queue_migrate
```

- [ ] **Step 2: point Solid Queue at the queue database in every environment**

In `config/application.rb`, inside `class Application`, add:

```ruby
    # Solid Queue is available in every environment (dev mirrors production
    # so the uploads-queue pause is testable locally); its schema lives in
    # the dedicated queue database.
    config.solid_queue.connects_to = { database: { writing: :queue } }
```

In `config/environments/production.rb`, delete the line
`config.solid_queue.connects_to = { database: { writing: :queue } }`
(keep `config.active_job.queue_adapter = :solid_queue`).

- [ ] **Step 3: dev runs jobs through Solid Queue inside Puma**

In `config/environments/development.rb`, add after the cache-store block:

```ruby
  # Mirror production: jobs go through Solid Queue (run inside Puma by
  # bin/dev) so the uploads-queue pause behaves the same locally.
  config.active_job.queue_adapter = :solid_queue
```

Replace `bin/dev` with:

```ruby
#!/usr/bin/env ruby
ENV["SOLID_QUEUE_IN_PUMA"] ||= "true"
exec "./bin/rails", "server", *ARGV
```

- [ ] **Step 4: prepare the databases and verify**

Run: `bin/rails db:prepare && bin/rails db:test:prepare`
Expected: `ged_development_queue` and `ged_test_queue` created and loaded from `db/queue_schema.rb` (verify with `bin/rails runner "puts SolidQueue::Job.count"` → `0`).

Run: `bin/rails test && bin/rubocop`
Expected: full suite still green (the `:test` adapter is untouched; parallel workers get per-worker queue databases automatically).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: run Solid Queue in development and test, mirroring production

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Dedicated uploads queue + admin pause/resume

**Files:**
- Create: `app/models/upload_queue.rb`
- Create: `app/controllers/admin/upload_queue_pauses_controller.rb`
- Create: `test/controllers/admin/upload_queue_pause_test.rb`
- Modify: `app/jobs/store_document_job.rb` (`queue_as :uploads`)
- Modify: `app/jobs/reconcile_upload_requests_job.rb` (skip pending re-enqueue while paused)
- Modify: `app/controllers/admin/upload_requests_controller.rb`, `app/views/admin/upload_requests/index.html.erb` (banner)
- Modify: `config/routes.rb`
- Test: `test/jobs/store_document_job_test.rb`, `test/jobs/reconcile_upload_requests_job_test.rb`

**Interfaces:**
- Consumes: `SolidQueue::Queue.find_by_name(name)` / `SolidQueue::Pause` (Solid Queue 1.4, available in test since Task 3).
- Produces: `UploadQueue.pause!` / `UploadQueue.resume!` / `UploadQueue.paused?` (module methods); routes `POST/DELETE /admin/upload_queue_pause`; `StoreDocumentJob.new.queue_name == "uploads"`.

- [ ] **Step 1: Write the failing tests**

`test/controllers/admin/upload_queue_pause_test.rb`:

```ruby
require "test_helper"

class Admin::UploadQueuePauseTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = Admin.create!(username: "alice", password: "secret123")
    sign_in @admin
    @finance = Department.create!(name: "Finance")
  end

  test "pauses and resumes the uploads queue" do
    post "/admin/upload_queue_pause"
    assert_redirected_to "/admin/upload_requests"
    assert UploadQueue.paused?
    assert SolidQueue::Pause.exists?(queue_name: "uploads")

    delete "/admin/upload_queue_pause"
    assert_redirected_to "/admin/upload_requests"
    assert_not UploadQueue.paused?
  end

  test "the banner reflects queue state and request counts" do
    UploadRequest.create!(department: @finance, filename: "r.pdf",
                          callback_url: "https://caller.test/cb",
                          uploaded_by: "erp-app", status: "pending")

    get "/admin/upload_requests"
    assert_response :success
    assert_includes response.body, "active"
    assert_includes response.body, "1 pending"

    UploadQueue.pause!
    get "/admin/upload_requests"
    assert_includes response.body, "paused"
    assert_includes response.body, "Resume"
  end

  test "requires an admin session" do
    sign_out @admin
    post "/admin/upload_queue_pause"
    assert_redirected_to "/admins/sign_in"
  end
end
```

In `test/jobs/store_document_job_test.rb`, add:

```ruby
  test "runs on the dedicated uploads queue" do
    assert_equal "uploads", StoreDocumentJob.new.queue_name
  end
```

In `test/jobs/reconcile_upload_requests_job_test.rb`, add (uses that file's
existing `create_request` helper):

```ruby
  test "does not re-enqueue stuck pending requests while the uploads queue is paused" do
    request = create_request(age: 20.minutes)
    StagingStore.write(request.id, StringIO.new("PDF!"))
    UploadQueue.pause!

    ReconcileUploadRequestsJob.perform_now

    assert_no_enqueued_jobs only: StoreDocumentJob
    assert_equal "pending", request.reload.status
  ensure
    UploadQueue.resume!
    StagingStore.delete(request.id)
  end
```

- [ ] **Step 2: Run them to verify they fail**

Run: `bin/rails test test/controllers/admin/upload_queue_pause_test.rb test/jobs/`
Expected: FAIL/ERROR (`UploadQueue` undefined, route missing, queue name `default`).

- [ ] **Step 3: Implement**

`app/models/upload_queue.rb`:

```ruby
# Facade over Solid Queue's native pause for the dedicated uploads queue
# (StoreDocumentJob). Pausing stops workers from claiming new store jobs; a
# job already running finishes and its callback (on :default) still goes out.
module UploadQueue
  NAME = "uploads"

  def self.pause!
    SolidQueue::Queue.find_by_name(NAME).pause
  end

  def self.resume!
    SolidQueue::Queue.find_by_name(NAME).resume
  end

  def self.paused?
    SolidQueue::Pause.exists?(queue_name: NAME)
  end
end
```

`app/jobs/store_document_job.rb` — change `queue_as :default` to:

```ruby
  queue_as :uploads
```

`app/jobs/reconcile_upload_requests_job.rb` — guard the pending recovery:

```ruby
  def recover_stuck_pending_requests
    # A paused uploads queue means pending requests are intentionally
    # waiting; re-enqueueing every run would pile duplicates into it.
    return if UploadQueue.paused?

    UploadRequest.where(status: "pending")
                 .where(created_at: ..STUCK_AFTER.ago).find_each do |request|
      # … body unchanged
```

`app/controllers/admin/upload_queue_pauses_controller.rb`:

```ruby
class Admin::UploadQueuePausesController < Admin::BaseController
  def create
    UploadQueue.pause!
    redirect_to admin_upload_requests_path, notice: "Upload queue paused."
  end

  def destroy
    UploadQueue.resume!
    redirect_to admin_upload_requests_path, notice: "Upload queue resumed."
  end
end
```

`config/routes.rb` — in the admin namespace, after `resources :upload_requests…`:

```ruby
    resource :upload_queue_pause, only: [ :create, :destroy ]
```

`app/controllers/admin/upload_requests_controller.rb`:

```ruby
class Admin::UploadRequestsController < Admin::BaseController
  def index
    scope = UploadRequest.includes(:department).order(created_at: :desc, id: :desc)
    scope = scope.where(status: params[:status]) if params[:status].present?
    @upload_requests = paginate(scope)
    @queue_paused = UploadQueue.paused?
    @pending_count = UploadRequest.where(status: "pending").count
    @processing_count = UploadRequest.where(status: "processing").count
  end
end
```

`app/views/admin/upload_requests/index.html.erb` — insert between the `<h1>`
and the filter form:

```erb
<p>
  Upload queue:
  <% if @queue_paused %>
    <strong>paused</strong>
    <%= button_to "Resume", admin_upload_queue_pause_path, method: :delete %>
  <% else %>
    active
    <%= button_to "Pause", admin_upload_queue_pause_path, method: :post %>
  <% end %>
  — <%= @pending_count %> pending, <%= @processing_count %> processing
</p>
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `bin/rails test test/controllers/admin/upload_queue_pause_test.rb test/jobs/`
Expected: PASS.

- [ ] **Step 5: Full suite + rubocop, then verify the pause live**

Run: `bin/rails test && bin/rubocop`
Expected: green.

Live check (dev): `bin/dev` in one terminal; in another,
`bin/rails runner 'UploadQueue.pause!'`, enqueue an async upload (or
`bin/rails runner 'StoreDocumentJob.perform_later(SecureRandom.uuid)'`),
confirm the job stays in `solid_queue_ready_executions`;
`bin/rails runner 'UploadQueue.resume!'` → it drains.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: dedicated uploads queue with admin pause/resume

Pausing stops claiming new store jobs; in-flight jobs finish and their
callbacks (default queue) still deliver. Reconcile skips pending
re-enqueues while paused.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Documentation + full CI

**Files:**
- Modify: `AGENTS.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Update AGENTS.md**

Apply these edits:

1. In the intro paragraph, change "departments (folders, the permission
   boundary) and cross-cutting tags" to "departments (folders, the
   permission boundary) and per-document key→value tags".
2. In **Key architectural decisions**, add a bullet:

```markdown
- **Tags are a key→value map, not a registry:** `documents.tags` is a
  single-level jsonb map (`{"type": "facture", "urgent": null}`, GIN-indexed),
  validated/normalized by the `KvTaggable` concern (also on
  `upload_requests.tags`). Tags are passed at upload (`tags[type]=facture`),
  replaced wholesale via `PUT /api/v1/documents/:id/tags`, and filtered with
  `?tag=<key>` / `?tag=<key>&tag_value=<value>`. There are no `/api/v1/tags`
  endpoints and no admin tags section.
```

3. In **How the pieces fit**, update the async flow bullet: `StoreDocumentJob`
   runs on the dedicated `uploads` queue and applies `upload_request.tags`;
   the callback payload is
   `{ request_id, status, document_id, error_message, tags }` (tags `null` on
   failure); `GET /api/v1/upload_requests/:id` mirrors it. Add a bullet:

```markdown
- **Upload queue pause:** `UploadQueue` (facade over Solid Queue's native
  pause) backs the Pause/Resume button on `/admin/upload_requests`. Pausing
  stops claiming new `StoreDocumentJob`s; running jobs finish and their
  callbacks (on `default`) still deliver. `ReconcileUploadRequestsJob` skips
  pending re-enqueues while paused. Solid Queue runs in every environment
  (dev: inside Puma via `bin/dev`, queue database `ged_development_queue`).
```

4. In the jobs bullet, change "dev/test use the default ActiveJob adapters"
   to "dev uses Solid Queue in Puma too; the test env keeps the `:test`
   adapter but has the queue schema loaded (`ged_test_queue`)".

- [ ] **Step 2: Full CI**

Run: `bin/ci`
Expected: main suite, system tests, rubocop, brakeman, bundler-audit all green. (`cd oauth-provider && bundle exec rake` is untouched by this work.)

- [ ] **Step 3: Commit**

```bash
git add AGENTS.md
git commit -m "docs: AGENTS.md — KV tag map, uploads queue pause, env changes

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```
