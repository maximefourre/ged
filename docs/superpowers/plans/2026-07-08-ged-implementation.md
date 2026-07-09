# GED Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the GED document management system — a Rails app exposing a JWT-secured document API with per-department permissions, a minimal admin UI, and a temporary Sinatra OAuth2 provider — per `docs/superpowers/specs/2026-07-08-ged-design.md`.

**Architecture:** Single Rails 8 app at the repo root (API-only controllers under `/api/v1`, ERB admin UI), PostgreSQL, Active Storage on local disk. A separate tiny Sinatra app in `oauth-provider/` issues RS256 client_credentials JWTs and serves a JWKS endpoint, standing in for the company IdP. Docker Compose ties `web` + `postgres` + `oauth-provider` together for deployment; dev/test runs natively with Homebrew Postgres.

**Tech Stack:** Ruby 3.4.x (via mise), Rails ~> 8.0, PostgreSQL 17 (Homebrew for dev), Devise, `jwt` gem, Minitest (Rails default), WebMock, Sinatra (oauth-provider), SQLite (oauth-provider only), Docker Compose (deployment).

## Global Constraints

- Ruby 3.4.x, Rails ~> 8.0, Minitest + WebMock for tests. RuboCop rails-omakase (Rails default) — run `bin/rubocop` before each commit.
- `documents` and `upload_requests` tables use UUID primary keys (`id: :uuid`, pgcrypto).
- Env vars (exact names): `GED_OIDC_ISSUER`, `GED_OIDC_JWKS_URL`, `GED_CLIENT_ID_CLAIM` (default `"sub"`), `GED_ADMIN_USERNAME` (default `"admin"`), `GED_ADMIN_PASSWORD` (default `"changeme"` in dev only).
- JWKS validation fails closed: if no key set has EVER been fetched, all bearer auth → 401. A previously fetched key set is reused stale when refresh fails.
- Error semantics: missing/invalid/expired JWT → 401; valid JWT but unregistered client_id, or no access to target department → 403; not found → 404; missing param → 400; validation failure → 422.
- Department access covers its whole subtree (granted department + all descendants).
- Async upload = same `POST /api/v1/documents` endpoint, triggered by presence of `callback_url` param; returns 202 with `request_id`; callback POST body: `{ request_id, status, document_id }`.
- Ports: Rails 3000, oauth-provider 9292.
- Commit after every task (at minimum). Do not commit `storage/`, `oauth-provider/data/`, or `*.sqlite3`.

---

### Task 1: Development environment + Rails scaffold

The machine has only system Ruby 2.6, no Rails, no Postgres, no Docker. This task installs the toolchain, generates the Rails app **at the repo root** (repo already contains `CLAUDE.md`, `docs/`, `.git` — they must survive), and proves the app boots.

**Files:**
- Create: entire Rails skeleton at repo root (`rails new`), `.mise.toml` (via mise), `test/smoke_test.rb` → actually `test/integration/health_check_test.rb`
- Preserve: `CLAUDE.md`, `docs/**`

**Interfaces:**
- Produces: bootable Rails app; `bin/rails test` green; Postgres running as a brew service.

- [x] **Step 1: Install mise + Ruby 3.4 + PostgreSQL 17**

```bash
brew install mise postgresql@17
brew services start postgresql@17
cd /Users/fourreto/Projects/vibecode/ged
mise use ruby@3.4        # writes .mise.toml, compiles Ruby (several minutes)
eval "$(mise activate bash --shims)"   # or ensure mise shims are on PATH for subsequent commands
ruby -v                  # expect: ruby 3.4.x
```

Note for the executor: every later `ruby`/`rails`/`bundle`/`bin/*` command in this plan assumes mise's Ruby is active (`mise x -- <cmd>` works if activation is unavailable in your shell).

- [x] **Step 2: Install Rails and generate the app in place**

```bash
gem install rails -v "~> 8.0"
rails new . --name=ged --database=postgresql \
  --skip-action-mailbox --skip-action-text --skip-action-cable \
  --skip-hotwire --skip-jbuilder --skip-kamal --skip-github --force
```

`--force` overwrites generated-file conflicts only; verify `CLAUDE.md` and `docs/` are untouched afterwards (`git status`).

- [x] **Step 3: Create databases and write the health-check test**

```bash
bin/rails db:create db:migrate
```

Create `test/integration/health_check_test.rb`:

```ruby
require "test_helper"

class HealthCheckTest < ActionDispatch::IntegrationTest
  test "GET /up returns 200" do
    get "/up"
    assert_response :success
  end
end
```

- [x] **Step 4: Run the test**

Run: `bin/rails test test/integration/health_check_test.rb`
Expected: 1 runs, 1 assertions, 0 failures

- [x] **Step 5: Add shared gems now (used from Task 2 on)**

In `Gemfile` add:

```ruby
gem "devise"
gem "jwt"

group :test do
  gem "webmock"
end
```

```bash
bundle install
```

In `test/test_helper.rb`, after `require "rails/test_help"` add:

```ruby
require "webmock/minitest"

ENV["GED_OIDC_ISSUER"] ||= "https://idp.test"
ENV["GED_OIDC_JWKS_URL"] ||= "https://idp.test/.well-known/jwks.json"
```

- [x] **Step 6: Verify full suite passes, then commit**

Run: `bin/rails test && bin/rubocop`
Expected: 0 failures, no offenses

```bash
git add -A
git status   # confirm CLAUDE.md/docs unchanged, no sqlite/storage junk
git commit -m "chore: scaffold Rails 8 app with postgres, devise/jwt/webmock gems"
```

---

### Task 2: Admin authentication (Devise, username-based)

**Files:**
- Create: `app/models/admin.rb`, migration `db/migrate/*_devise_create_admins.rb`, `app/controllers/admin/base_controller.rb`, `app/views/layouts/admin.html.erb`, devise session views, `test/models/admin_test.rb`, `test/controllers/admin/authentication_test.rb`
- Modify: `config/routes.rb`, `config/initializers/devise.rb`, `db/seeds.rb`

**Interfaces:**
- Produces: `Admin` model (`username`, Devise `:database_authenticatable`); `Admin::BaseController` with `before_action :authenticate_admin!` for all later admin controllers; seeded admin from `GED_ADMIN_USERNAME`/`GED_ADMIN_PASSWORD`.

- [x] **Step 1: Install Devise and generate the Admin model**

```bash
bin/rails generate devise:install
bin/rails generate devise Admin
```

Edit the generated migration to use `username` instead of `email` (keep only what we need):

```ruby
class DeviseCreateAdmins < ActiveRecord::Migration[8.0]
  def change
    create_table :admins do |t|
      t.string :username, null: false
      t.string :encrypted_password, null: false, default: ""
      t.timestamps null: false
    end
    add_index :admins, :username, unique: true
  end
end
```

Edit `app/models/admin.rb`:

```ruby
class Admin < ApplicationRecord
  devise :database_authenticatable

  validates :username, presence: true, uniqueness: { case_sensitive: false }
end
```

In `config/initializers/devise.rb` set:

```ruby
config.authentication_keys = [:username]
config.case_insensitive_keys = [:username]
config.strip_whitespace_keys = [:username]
```

Run: `bin/rails db:migrate`

- [x] **Step 2: Write failing tests**

`test/models/admin_test.rb`:

```ruby
require "test_helper"

class AdminTest < ActiveSupport::TestCase
  test "requires unique username" do
    Admin.create!(username: "alice", password: "secret123")
    dup = Admin.new(username: "ALICE", password: "secret123")
    assert_not dup.valid?
  end

  test "authenticates with valid password" do
    admin = Admin.create!(username: "bob", password: "secret123")
    assert admin.valid_password?("secret123")
    assert_not admin.valid_password?("wrong")
  end
end
```

`test/controllers/admin/authentication_test.rb`:

```ruby
require "test_helper"

class Admin::AuthenticationTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  test "unauthenticated access to admin area redirects to login" do
    get "/admin"
    assert_redirected_to new_admin_session_path
  end

  test "authenticated admin reaches admin area" do
    admin = Admin.create!(username: "alice", password: "secret123")
    sign_in admin
    get "/admin"
    assert_response :success
  end
end
```

Delete the generated `test/fixtures/admins.yml` (fixtures with empty rows break `encrypted_password` constraints):

```bash
rm -f test/fixtures/admins.yml
```

Run: `bin/rails test`
Expected: FAIL (no `/admin` route, controller missing)

- [x] **Step 3: Implement routes, base controller, layout, placeholder dashboard**

`config/routes.rb`:

```ruby
Rails.application.routes.draw do
  devise_for :admins
  get "up" => "rails/health#show", as: :rails_health_check

  namespace :admin do
    root to: "dashboard#index"
  end
  root to: redirect("/admin")
end
```

`app/controllers/admin/base_controller.rb`:

```ruby
class Admin::BaseController < ApplicationController
  before_action :authenticate_admin!
  layout "admin"
end
```

`app/controllers/admin/dashboard_controller.rb`:

```ruby
class Admin::DashboardController < Admin::BaseController
  def index
  end
end
```

`app/views/admin/dashboard/index.html.erb`:

```erb
<h1>GED Admin</h1>
```

`app/views/layouts/admin.html.erb`:

```erb
<!DOCTYPE html>
<html>
  <head>
    <title>GED Admin</title>
    <%= csrf_meta_tags %>
    <%= stylesheet_link_tag "application" %>
  </head>
  <body>
    <nav>
      <%= link_to "Documents", "/admin/documents" %> |
      <%= link_to "Departments", "/admin/departments" %> |
      <%= link_to "Tags", "/admin/tags" %> |
      <%= link_to "Clients", "/admin/app_clients" %> |
      <% if admin_signed_in? %>
        <%= button_to "Log out", destroy_admin_session_path, method: :delete %>
      <% end %>
    </nav>
    <% flash.each do |kind, msg| %><p class="flash-<%= kind %>"><%= msg %></p><% end %>
    <%= yield %>
  </body>
</html>
```

Generate and adapt the login view:

```bash
bin/rails generate devise:views
```

Replace `app/views/devise/sessions/new.html.erb` content:

```erb
<h2>Log in</h2>
<%= form_for(resource, as: resource_name, url: session_path(resource_name)) do |f| %>
  <div><%= f.label :username %> <%= f.text_field :username, autofocus: true %></div>
  <div><%= f.label :password %> <%= f.password_field :password %></div>
  <div><%= f.submit "Log in" %></div>
<% end %>
```

`db/seeds.rb`:

```ruby
Admin.find_or_create_by!(username: ENV.fetch("GED_ADMIN_USERNAME", "admin")) do |admin|
  admin.password = ENV.fetch("GED_ADMIN_PASSWORD", "changeme")
end
```

- [x] **Step 4: Run tests**

Run: `bin/rails test`
Expected: PASS (all)

- [x] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: admin authentication with Devise (username/password)"
```

---

### Task 3: Department model (folder hierarchy, permission boundary)

**Files:**
- Create: migration `*_create_departments.rb`, `app/models/department.rb`, `test/models/department_test.rb`

**Interfaces:**
- Produces: `Department` (`name`, optional `parent`), `Department#self_and_ancestors → Array<Department>` (self first, root last), `Department#subtree_ids → Array<Integer>` (self + all descendants). Later tasks (6, 9, 10) rely on these exact names.

- [x] **Step 1: Write failing tests**

`test/models/department_test.rb`:

```ruby
require "test_helper"

class DepartmentTest < ActiveSupport::TestCase
  test "requires a name, unique among siblings" do
    root = Department.create!(name: "Finance")
    assert_not Department.new(name: "", parent: root).valid?
    assert_not Department.new(name: "Finance").valid?          # duplicate at root
    assert Department.new(name: "Finance", parent: root).valid? # same name under a parent is fine
  end

  test "self_and_ancestors walks up to the root" do
    root = Department.create!(name: "Finance")
    child = Department.create!(name: "Invoices", parent: root)
    grandchild = Department.create!(name: "2026", parent: child)
    assert_equal [grandchild, child, root], grandchild.self_and_ancestors
  end

  test "subtree_ids includes self and all descendants" do
    root = Department.create!(name: "Finance")
    child = Department.create!(name: "Invoices", parent: root)
    grandchild = Department.create!(name: "2026", parent: child)
    other = Department.create!(name: "HR")
    assert_equal [root.id, child.id, grandchild.id].sort, root.subtree_ids.sort
    assert_equal [other.id], other.subtree_ids
  end

  test "parent cannot be self or a descendant" do
    root = Department.create!(name: "Finance")
    child = Department.create!(name: "Invoices", parent: root)
    root.parent = child
    assert_not root.valid?
    child.parent = child
    assert_not child.valid?
  end
end
```

Run: `bin/rails test test/models/department_test.rb`
Expected: FAIL (table missing)

- [x] **Step 2: Migration + model**

```bash
bin/rails generate migration CreateDepartments
```

```ruby
class CreateDepartments < ActiveRecord::Migration[8.0]
  def change
    create_table :departments do |t|
      t.string :name, null: false
      t.references :parent, foreign_key: { to_table: :departments }
      t.timestamps
    end
    add_index :departments, [:parent_id, :name], unique: true
    add_index :departments, :name, unique: true, where: "parent_id IS NULL",
              name: "index_departments_on_name_at_root"
  end
end
```

`app/models/department.rb`:

```ruby
class Department < ApplicationRecord
  belongs_to :parent, class_name: "Department", optional: true
  has_many :children, class_name: "Department", foreign_key: :parent_id,
           dependent: :restrict_with_error

  validates :name, presence: true, uniqueness: { scope: :parent_id }
  validate :parent_is_not_self_or_descendant

  def self_and_ancestors
    node, list, seen = self, [], Set.new
    while node && seen.exclude?(node.id)
      list << node
      seen << node.id
      node = node.parent
    end
    list
  end

  def subtree_ids
    ids = [id]
    frontier = [id]
    while frontier.any?
      frontier = Department.where(parent_id: frontier).pluck(:id)
      ids.concat(frontier)
    end
    ids
  end

  private

  def parent_is_not_self_or_descendant
    return if parent_id.blank? || !persisted?
    errors.add(:parent_id, "cannot be self or a descendant") if subtree_ids.include?(parent_id)
  end
end
```

Run: `bin/rails db:migrate`

- [x] **Step 3: Run tests**

Run: `bin/rails test test/models/department_test.rb`
Expected: PASS (4 tests)

- [x] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: Department model with hierarchy and subtree helpers"
```

---

### Task 4: Tag and DocumentTag models

**Files:**
- Create: migration `*_create_tags.rb`, `app/models/tag.rb`, `test/models/tag_test.rb`. (The `document_tags` join table ships in Task 5 with `documents`, since it needs the UUID column type.)

**Interfaces:**
- Produces: `Tag` (`name`, normalized lowercase, unique).

- [x] **Step 1: Write failing tests**

`test/models/tag_test.rb`:

```ruby
require "test_helper"

class TagTest < ActiveSupport::TestCase
  test "normalizes name to lowercase and strips whitespace" do
    tag = Tag.create!(name: "  Invoice ")
    assert_equal "invoice", tag.name
  end

  test "name is required and unique after normalization" do
    Tag.create!(name: "invoice")
    assert_not Tag.new(name: "INVOICE").valid?
    assert_not Tag.new(name: "").valid?
  end
end
```

Run: `bin/rails test test/models/tag_test.rb`
Expected: FAIL (table missing)

- [x] **Step 2: Migration + model**

```bash
bin/rails generate migration CreateTags
```

```ruby
class CreateTags < ActiveRecord::Migration[8.0]
  def change
    create_table :tags do |t|
      t.string :name, null: false
      t.timestamps
    end
    add_index :tags, :name, unique: true
  end
end
```

`app/models/tag.rb`:

```ruby
class Tag < ApplicationRecord
  normalizes :name, with: ->(name) { name.strip.downcase }

  validates :name, presence: true, uniqueness: true
end
```

Run: `bin/rails db:migrate`

- [x] **Step 3: Run tests, commit**

Run: `bin/rails test test/models/tag_test.rb`
Expected: PASS

```bash
git add -A && git commit -m "feat: Tag model with normalized unique names"
```

---

### Task 5: Document model + Active Storage (UUID primary key)

**Files:**
- Create: migrations `*_enable_pgcrypto.rb`, `*_create_documents.rb` (includes `document_tags`), `app/models/document.rb`, `app/models/document_tag.rb`, `test/models/document_test.rb`
- Modify: `app/models/tag.rb` (add associations)

**Interfaces:**
- Produces: `Document` (UUID id, `filename`, `content_type`, `byte_size`, `department`, `uploaded_by`, `has_one_attached :file`, `has_many :tags`). `Tag#documents`. Later tasks rely on `Document#file` and these column names.

- [x] **Step 1: Install Active Storage, write failing tests**

```bash
bin/rails active_storage:install
```

`test/models/document_test.rb`:

```ruby
require "test_helper"

class DocumentTest < ActiveSupport::TestCase
  setup do
    @department = Department.create!(name: "Finance")
  end

  def build_document
    doc = Document.new(department: @department, filename: "report.pdf",
                       content_type: "application/pdf", byte_size: 4,
                       uploaded_by: "client-a")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc
  end

  test "has a uuid primary key and stores a file" do
    doc = build_document
    doc.save!
    assert_match(/\A[0-9a-f-]{36}\z/, doc.id)
    assert doc.file.attached?
  end

  test "requires filename, department, uploaded_by" do
    assert_not Document.new.valid?
    doc = build_document
    doc.filename = nil
    assert_not doc.valid?
  end

  test "can be tagged" do
    doc = build_document
    doc.save!
    doc.tags << Tag.create!(name: "invoice")
    assert_equal ["invoice"], doc.tags.pluck(:name)
  end
end
```

Run: `bin/rails test test/models/document_test.rb`
Expected: FAIL (table missing)

- [x] **Step 2: Migrations + models**

```bash
bin/rails generate migration EnablePgcrypto
bin/rails generate migration CreateDocuments
```

```ruby
class EnablePgcrypto < ActiveRecord::Migration[8.0]
  def change
    enable_extension "pgcrypto"
  end
end
```

```ruby
class CreateDocuments < ActiveRecord::Migration[8.0]
  def change
    create_table :documents, id: :uuid do |t|
      t.string :filename, null: false
      t.string :content_type
      t.bigint :byte_size
      t.references :department, null: false, foreign_key: true
      t.string :uploaded_by, null: false
      t.timestamps
    end

    create_table :document_tags do |t|
      t.references :document, null: false, foreign_key: true, type: :uuid
      t.references :tag, null: false, foreign_key: true
      t.timestamps
    end
    add_index :document_tags, [:document_id, :tag_id], unique: true
  end
end
```

`app/models/document.rb`:

```ruby
class Document < ApplicationRecord
  belongs_to :department
  has_one_attached :file
  has_many :document_tags, dependent: :destroy
  has_many :tags, through: :document_tags

  validates :filename, :uploaded_by, presence: true
end
```

`app/models/document_tag.rb`:

```ruby
class DocumentTag < ApplicationRecord
  belongs_to :document
  belongs_to :tag
end
```

Add to `app/models/tag.rb` (inside the class):

```ruby
  has_many :document_tags, dependent: :destroy
  has_many :documents, through: :document_tags
```

Add to `app/models/department.rb` (inside the class, with the other associations):

```ruby
  has_many :documents, dependent: :restrict_with_error
```

Run: `bin/rails db:migrate`

- [x] **Step 3: Run tests, commit**

Run: `bin/rails test`
Expected: PASS (all)

```bash
git add -A && git commit -m "feat: Document model with Active Storage, uuid pk, tagging"
```

---

### Task 6: AppClient + department access mapping

**Files:**
- Create: migration `*_create_app_clients.rb`, `app/models/app_client.rb`, `app/models/client_department_access.rb`, `test/models/app_client_test.rb`

**Interfaces:**
- Produces: `AppClient` (`client_identifier` unique, `description`), `AppClient#can_access?(department) → Boolean` (true if any granted department is in `department.self_and_ancestors`), `AppClient#accessible_department_ids → Array<Integer>` (all granted subtrees, for index filtering), `AppClient#departments` (granted departments association). Tasks 8–11 rely on these exact names.

- [x] **Step 1: Write failing tests**

`test/models/app_client_test.rb`:

```ruby
require "test_helper"

class AppClientTest < ActiveSupport::TestCase
  setup do
    @finance = Department.create!(name: "Finance")
    @invoices = Department.create!(name: "Invoices", parent: @finance)
    @hr = Department.create!(name: "HR")
    @client = AppClient.create!(client_identifier: "erp-app", description: "ERP")
    @client.departments << @finance
  end

  test "client_identifier is required and unique" do
    assert_not AppClient.new.valid?
    assert_not AppClient.new(client_identifier: "erp-app").valid?
  end

  test "can_access? covers the granted department and its subtree" do
    assert @client.can_access?(@finance)
    assert @client.can_access?(@invoices)   # subtree
    assert_not @client.can_access?(@hr)
  end

  test "accessible_department_ids expands granted subtrees" do
    assert_equal [@finance.id, @invoices.id].sort, @client.accessible_department_ids.sort
  end
end
```

Run: `bin/rails test test/models/app_client_test.rb`
Expected: FAIL (table missing)

- [x] **Step 2: Migration + models**

```bash
bin/rails generate migration CreateAppClients
```

```ruby
class CreateAppClients < ActiveRecord::Migration[8.0]
  def change
    create_table :app_clients do |t|
      t.string :client_identifier, null: false
      t.string :description
      t.timestamps
    end
    add_index :app_clients, :client_identifier, unique: true

    create_table :client_department_accesses do |t|
      t.references :app_client, null: false, foreign_key: true
      t.references :department, null: false, foreign_key: true
      t.timestamps
    end
    add_index :client_department_accesses, [:app_client_id, :department_id],
              unique: true, name: "index_client_department_accesses_uniqueness"
  end
end
```

`app/models/app_client.rb`:

```ruby
class AppClient < ApplicationRecord
  has_many :client_department_accesses, dependent: :destroy
  has_many :departments, through: :client_department_accesses

  validates :client_identifier, presence: true, uniqueness: true

  def can_access?(department)
    granted = department_ids
    department.self_and_ancestors.any? { |dept| granted.include?(dept.id) }
  end

  def accessible_department_ids
    departments.flat_map(&:subtree_ids).uniq
  end
end
```

`app/models/client_department_access.rb`:

```ruby
class ClientDepartmentAccess < ApplicationRecord
  belongs_to :app_client
  belongs_to :department
end
```

Run: `bin/rails db:migrate`

- [x] **Step 3: Run tests, commit**

Run: `bin/rails test`
Expected: PASS

```bash
git add -A && git commit -m "feat: AppClient with per-department access mapping"
```

---

### Task 7: JWT validation services (JWKS cache + token validator)

**Files:**
- Create: `app/services/auth/jwks_cache.rb`, `app/services/auth/token_validator.rb`, `test/services/auth/token_validator_test.rb`, `test/support/jwt_test_helpers.rb`
- Modify: `test/test_helper.rb` (require support files)

**Interfaces:**
- Produces:
  - `Auth::JwksCache.instance.key_set → JWT::JWK::Set` (raises `Auth::JwksCache::FetchError` only when nothing was ever fetched); `Auth::JwksCache.reset!` (test hook).
  - `Auth::TokenValidator.call(token) → Auth::TokenValidator::Result` with `#client_identifier`; raises `Auth::TokenValidator::InvalidToken` on any validation failure.
  - Test helper module `JwtTestHelpers`: `issue_token(client_id:, iss:, exp:)`, `stub_jwks!`, `bearer_header(client_id)` — used by all API tests in Tasks 8–11.

- [x] **Step 1: Write the test helper and failing tests**

`test/support/jwt_test_helpers.rb`:

```ruby
module JwtTestHelpers
  SIGNING_KEY = OpenSSL::PKey::RSA.new(2048)
  SIGNING_JWK = JWT::JWK.new(SIGNING_KEY, { use: "sig", alg: "RS256" })

  def stub_jwks!
    stub_request(:get, ENV.fetch("GED_OIDC_JWKS_URL"))
      .to_return(status: 200, body: { keys: [SIGNING_JWK.export] }.to_json,
                 headers: { "Content-Type" => "application/json" })
  end

  def issue_token(client_id:, iss: ENV.fetch("GED_OIDC_ISSUER"), exp: 1.hour.from_now.to_i)
    payload = { sub: client_id, iss: iss, exp: exp, iat: Time.now.to_i }
    JWT.encode(payload, SIGNING_KEY, "RS256", kid: SIGNING_JWK.kid)
  end

  def bearer_header(client_id)
    { "Authorization" => "Bearer #{issue_token(client_id: client_id)}" }
  end
end
```

In `test/test_helper.rb` add (below the ENV defaults):

```ruby
Dir[Rails.root.join("test/support/**/*.rb")].sort.each { |f| require f }
```

And inside `class TestCase` (module `ActiveSupport`), add:

```ruby
    include JwtTestHelpers

    setup { Auth::JwksCache.reset! }
```

`test/services/auth/token_validator_test.rb`:

```ruby
require "test_helper"

class Auth::TokenValidatorTest < ActiveSupport::TestCase
  test "returns the client identifier from a valid token" do
    stub_jwks!
    result = Auth::TokenValidator.call(issue_token(client_id: "erp-app"))
    assert_equal "erp-app", result.client_identifier
  end

  test "rejects an expired token" do
    stub_jwks!
    token = issue_token(client_id: "erp-app", exp: 1.hour.ago.to_i)
    assert_raises(Auth::TokenValidator::InvalidToken) { Auth::TokenValidator.call(token) }
  end

  test "rejects a token with the wrong issuer" do
    stub_jwks!
    token = issue_token(client_id: "erp-app", iss: "https://evil.test")
    assert_raises(Auth::TokenValidator::InvalidToken) { Auth::TokenValidator.call(token) }
  end

  test "rejects a token signed by an unknown key" do
    stub_jwks!
    other_key = OpenSSL::PKey::RSA.new(2048)
    token = JWT.encode({ sub: "erp-app", iss: ENV["GED_OIDC_ISSUER"],
                         exp: 1.hour.from_now.to_i }, other_key, "RS256", kid: "unknown")
    assert_raises(Auth::TokenValidator::InvalidToken) { Auth::TokenValidator.call(token) }
  end

  test "fails closed when JWKS was never fetched" do
    stub_request(:get, ENV.fetch("GED_OIDC_JWKS_URL")).to_return(status: 500)
    assert_raises(Auth::TokenValidator::InvalidToken) do
      Auth::TokenValidator.call(issue_token(client_id: "erp-app"))
    end
  end

  test "keeps serving stale keys when a refresh fails" do
    stub_jwks!
    Auth::JwksCache.instance.key_set  # prime the cache
    stub_request(:get, ENV.fetch("GED_OIDC_JWKS_URL")).to_return(status: 500)
    Auth::JwksCache.instance.force_expire_for_tests!
    result = Auth::TokenValidator.call(issue_token(client_id: "erp-app"))
    assert_equal "erp-app", result.client_identifier
  end
end
```

Run: `bin/rails test test/services`
Expected: FAIL (constants missing)

- [x] **Step 2: Implement the services**

`app/services/auth/jwks_cache.rb`:

```ruby
require "net/http"

module Auth
  # Process-local JWKS cache. Refreshes every TTL seconds; keeps serving the
  # last good key set if a refresh fails; fails closed (FetchError) only if
  # no key set has ever been fetched.
  class JwksCache
    class FetchError < StandardError; end

    TTL = 300 # seconds

    class << self
      def instance
        @instance ||= new(ENV.fetch("GED_OIDC_JWKS_URL"))
      end

      def reset!
        @instance = nil
      end
    end

    def initialize(url)
      @url = url
      @mutex = Mutex.new
      @key_set = nil
      @fetched_at = nil
    end

    def key_set
      @mutex.synchronize do
        refresh if @key_set.nil? || stale?
        raise FetchError, "no JWKS available" if @key_set.nil?
        @key_set
      end
    end

    def force_expire_for_tests!
      @fetched_at = Time.at(0)
    end

    private

    def stale?
      @fetched_at.nil? || (Time.now - @fetched_at) > TTL
    end

    def refresh
      response = Net::HTTP.get_response(URI(@url))
      unless response.is_a?(Net::HTTPSuccess)
        raise FetchError, "JWKS endpoint returned #{response.code}"
      end
      @key_set = JWT::JWK::Set.new(JSON.parse(response.body))
      @fetched_at = Time.now
    rescue FetchError, StandardError => e
      Rails.logger.error("JWKS refresh failed: #{e.class}: #{e.message}")
      # keep stale keys if we have them; key_set raises if we never had any
    end
  end
end
```

`app/services/auth/token_validator.rb`:

```ruby
module Auth
  class TokenValidator
    class InvalidToken < StandardError; end

    Result = Struct.new(:client_identifier)

    def self.call(token)
      key_set = JwksCache.instance.key_set
      payload, = JWT.decode(
        token, nil, true,
        algorithms: ["RS256"],
        iss: ENV.fetch("GED_OIDC_ISSUER"),
        verify_iss: true,
        verify_expiration: true,
        jwks: key_set
      )
      claim = ENV.fetch("GED_CLIENT_ID_CLAIM", "sub")
      identifier = payload[claim]
      raise InvalidToken, "token missing #{claim} claim" if identifier.blank?
      Result.new(identifier)
    rescue JWT::DecodeError, JwksCache::FetchError => e
      raise InvalidToken, e.message
    end
  end
end
```

- [x] **Step 3: Run tests**

Run: `bin/rails test test/services`
Expected: PASS (6 tests)

- [x] **Step 4: Run the full suite, commit**

Run: `bin/rails test && bin/rubocop`
Expected: PASS, no offenses

```bash
git add -A && git commit -m "feat: JWKS cache and JWT token validator (fail-closed)"
```

---

### Task 8: API base controller with bearer authentication

**Files:**
- Create: `app/controllers/api/v1/base_controller.rb`, `app/controllers/api/v1/pings_controller.rb` (tiny probe endpoint used only to test the auth plumbing), `test/controllers/api/v1/authentication_test.rb`
- Modify: `config/routes.rb`

**Interfaces:**
- Consumes: `Auth::TokenValidator.call`, `AppClient.can_access?`.
- Produces: `Api::V1::BaseController` with `current_app_client → AppClient` and `authorize_department!(department)` (raises internal `NotAuthorizedError` → 403). Standard rescues: `ActiveRecord::RecordNotFound` → 404 `{error}`, `ActionController::ParameterMissing` → 400, `ActiveRecord::RecordInvalid` → 422. All Task 9–11 controllers inherit from this.

- [x] **Step 1: Write failing tests**

`test/controllers/api/v1/authentication_test.rb`:

```ruby
require "test_helper"

class Api::V1::AuthenticationTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @client = AppClient.create!(client_identifier: "erp-app")
  end

  test "no Authorization header returns 401" do
    get "/api/v1/ping"
    assert_response :unauthorized
  end

  test "garbage token returns 401" do
    get "/api/v1/ping", headers: { "Authorization" => "Bearer nope" }
    assert_response :unauthorized
  end

  test "expired token returns 401" do
    token = issue_token(client_id: "erp-app", exp: 1.hour.ago.to_i)
    get "/api/v1/ping", headers: { "Authorization" => "Bearer #{token}" }
    assert_response :unauthorized
  end

  test "valid token for an unregistered client returns 403" do
    get "/api/v1/ping", headers: bearer_header("unknown-app")
    assert_response :forbidden
  end

  test "valid token for a registered client returns 200" do
    get "/api/v1/ping", headers: bearer_header("erp-app")
    assert_response :success
    assert_equal "erp-app", JSON.parse(response.body)["client"]
  end
end
```

Run: `bin/rails test test/controllers/api`
Expected: FAIL (no route)

- [x] **Step 2: Implement**

`app/controllers/api/v1/base_controller.rb`:

```ruby
module Api
  module V1
    class BaseController < ActionController::API
      class NotAuthorizedError < StandardError; end

      before_action :authenticate_app_client!

      rescue_from NotAuthorizedError do |e|
        render json: { error: e.message.presence || "forbidden" }, status: :forbidden
      end
      rescue_from ActiveRecord::RecordNotFound do
        render json: { error: "not found" }, status: :not_found
      end
      rescue_from ActionController::ParameterMissing do |e|
        render json: { error: e.message }, status: :bad_request
      end
      rescue_from ActiveRecord::RecordInvalid do |e|
        render json: { error: e.record.errors.full_messages.join(", ") },
               status: :unprocessable_entity
      end

      private

      attr_reader :current_app_client

      def authenticate_app_client!
        token = request.headers["Authorization"]&.match(/\ABearer (.+)\z/)&.captures&.first
        return render_unauthorized("missing bearer token") if token.blank?

        result = Auth::TokenValidator.call(token)
        @current_app_client = AppClient.find_by(client_identifier: result.client_identifier)
        unless @current_app_client
          render json: { error: "client not registered" }, status: :forbidden
        end
      rescue Auth::TokenValidator::InvalidToken => e
        render_unauthorized(e.message)
      end

      def render_unauthorized(message)
        render json: { error: message }, status: :unauthorized
      end

      def authorize_department!(department)
        unless current_app_client.can_access?(department)
          raise NotAuthorizedError, "no access to this department"
        end
      end
    end
  end
end
```

`app/controllers/api/v1/pings_controller.rb`:

```ruby
module Api
  module V1
    class PingsController < BaseController
      def show
        render json: { client: current_app_client.client_identifier }
      end
    end
  end
end
```

In `config/routes.rb`, add inside `Rails.application.routes.draw`:

```ruby
  namespace :api do
    namespace :v1 do
      get "ping", to: "pings#show"
    end
  end
```

- [x] **Step 3: Run tests, commit**

Run: `bin/rails test`
Expected: PASS

```bash
git add -A && git commit -m "feat: API bearer-token authentication plumbing"
```

---

### Task 9: Documents API — synchronous upload, list, download, delete

**Files:**
- Create: `app/controllers/api/v1/documents_controller.rb`, `test/controllers/api/v1/documents_test.rb`
- Modify: `config/routes.rb`

**Interfaces:**
- Consumes: `authorize_department!`, `current_app_client.accessible_department_ids`, `Department#subtree_ids`.
- Produces: REST endpoints under `/api/v1/documents`. Document JSON shape (used again in Tasks 10–12): `{ id, filename, content_type, byte_size, department_id, uploaded_by, tags: [names], created_at }`. Task 11 extends `create` with the async branch — keep `create` structured as shown (guard clause at top) so the async branch slots in.

- [x] **Step 1: Write failing tests**

`test/controllers/api/v1/documents_test.rb`:

```ruby
require "test_helper"

class Api::V1::DocumentsTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @invoices = Department.create!(name: "Invoices", parent: @finance)
    @hr = Department.create!(name: "HR")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance
  end

  def upload_file
    fixture_file_upload("report.pdf", "application/pdf")
  end

  test "sync upload returns 201 with the document uuid" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id, file: upload_file }
    assert_response :created
    body = JSON.parse(response.body)
    assert_match(/\A[0-9a-f-]{36}\z/, body["id"])
    assert_equal "report.pdf", body["filename"]
    assert_equal @finance.id, body["department_id"]
    assert_equal "erp-app", body["uploaded_by"]
    assert Document.find(body["id"]).file.attached?
  end

  test "upload into a subfolder of a granted department is allowed" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @invoices.id, file: upload_file }
    assert_response :created
  end

  test "upload into a forbidden department returns 403" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @hr.id, file: upload_file }
    assert_response :forbidden
  end

  test "upload without file returns 400" do
    post "/api/v1/documents", headers: bearer_header("erp-app"),
         params: { department_id: @finance.id }
    assert_response :bad_request
  end

  test "list returns only accessible documents, with filters" do
    doc = create_document(@invoices, "facture-2026.pdf")
    create_document(@hr, "salaries.pdf")
    doc.tags << Tag.create!(name: "invoice")

    get "/api/v1/documents", headers: bearer_header("erp-app")
    names = JSON.parse(response.body).map { |d| d["filename"] }
    assert_equal ["facture-2026.pdf"], names

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

  test "filtering by a forbidden department returns 403" do
    get "/api/v1/documents", headers: bearer_header("erp-app"),
        params: { department_id: @hr.id }
    assert_response :forbidden
  end

  test "download streams the file content" do
    doc = create_document(@finance, "report.pdf")
    get "/api/v1/documents/#{doc.id}/download", headers: bearer_header("erp-app")
    assert_response :success
    assert_equal "PDF!", response.body
  end

  test "show, download, delete of a forbidden document return 403" do
    doc = create_document(@hr, "salaries.pdf")
    get "/api/v1/documents/#{doc.id}", headers: bearer_header("erp-app")
    assert_response :forbidden
    get "/api/v1/documents/#{doc.id}/download", headers: bearer_header("erp-app")
    assert_response :forbidden
    delete "/api/v1/documents/#{doc.id}", headers: bearer_header("erp-app")
    assert_response :forbidden
  end

  test "delete removes the document" do
    doc = create_document(@finance, "report.pdf")
    delete "/api/v1/documents/#{doc.id}", headers: bearer_header("erp-app")
    assert_response :no_content
    assert_not Document.exists?(doc.id)
  end

  test "unknown document returns 404" do
    get "/api/v1/documents/#{SecureRandom.uuid}", headers: bearer_header("erp-app")
    assert_response :not_found
  end

  private

  def create_document(department, filename)
    doc = Document.new(department: department, filename: filename,
                       content_type: "application/pdf", byte_size: 4,
                       uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: filename,
                    content_type: "application/pdf")
    doc.save!
    doc
  end
end
```

Create the fixture file:

```bash
mkdir -p test/fixtures/files
printf 'PDF!' > test/fixtures/files/report.pdf
```

Run: `bin/rails test test/controllers/api/v1/documents_test.rb`
Expected: FAIL (no routes)

- [x] **Step 2: Implement controller + routes**

`app/controllers/api/v1/documents_controller.rb`:

```ruby
module Api
  module V1
    class DocumentsController < BaseController
      def index
        scope = Document.where(department_id: current_app_client.accessible_department_ids)
        if params[:department_id].present?
          department = Department.find(params[:department_id])
          authorize_department!(department)
          scope = scope.where(department_id: department.subtree_ids)
        end
        if params[:filename].present?
          scope = scope.where("filename ILIKE ?",
                              "%#{Document.sanitize_sql_like(params[:filename])}%")
        end
        scope = scope.joins(:tags).where(tags: { name: params[:tag] }) if params[:tag].present?
        render json: scope.includes(:tags).order(created_at: :desc).map { |d| document_json(d) }
      end

      def create
        department = Department.find(params.require(:department_id))
        authorize_department!(department)
        file = params.require(:file)

        document = Document.new(
          department: department,
          filename: file.original_filename,
          content_type: file.content_type,
          byte_size: file.size,
          uploaded_by: current_app_client.client_identifier
        )
        document.file.attach(io: file.tempfile, filename: file.original_filename,
                             content_type: file.content_type)
        document.save!
        render json: document_json(document), status: :created
      end

      def show
        document = find_authorized_document
        render json: document_json(document)
      end

      def download
        document = find_authorized_document
        send_data document.file.download,
                  filename: document.filename,
                  content_type: document.content_type || "application/octet-stream"
      end

      def destroy
        document = find_authorized_document
        document.destroy!
        head :no_content
      end

      private

      def find_authorized_document
        document = Document.find(params[:id])
        authorize_department!(document.department)
        document
      end

      def document_json(document)
        {
          id: document.id,
          filename: document.filename,
          content_type: document.content_type,
          byte_size: document.byte_size,
          department_id: document.department_id,
          uploaded_by: document.uploaded_by,
          tags: document.tags.map(&:name),
          created_at: document.created_at
        }
      end
    end
  end
end
```

In `config/routes.rb`, replace the `namespace :api` block:

```ruby
  namespace :api do
    namespace :v1 do
      get "ping", to: "pings#show"
      resources :documents, only: [:index, :create, :show, :destroy] do
        member { get :download }
      end
    end
  end
```

- [x] **Step 3: Run tests, commit**

Run: `bin/rails test`
Expected: PASS

```bash
git add -A && git commit -m "feat: documents API with sync upload and department authorization"
```

> **Executor note (Task 9):** the Active Storage install migration created
> `active_storage_attachments.record_id` as bigint, which silently
> integer-cast Document uuid ids (flaky `attached?` failures). Added
> corrective migration `ChangeActiveStorageRecordIdToUuid` (wipes AS rows,
> converts record_id to uuid).

---

### Task 10: Departments API + Tags API (incl. tag assignment)

**Files:**
- Create: `app/controllers/api/v1/departments_controller.rb`, `app/controllers/api/v1/tags_controller.rb`, `app/controllers/api/v1/document_tags_controller.rb`, `test/controllers/api/v1/departments_test.rb`, `test/controllers/api/v1/tags_test.rb`
- Modify: `config/routes.rb`

**Interfaces:**
- Consumes: Task 8 base controller, Task 9 document JSON shape.
- Produces: `GET/POST /api/v1/departments` (list = accessible only; create = subfolder under an accessible parent), `GET/POST /api/v1/tags`, `POST /api/v1/documents/:document_id/tags` (assign existing tag by `name`). Department JSON: `{ id, name, parent_id }`.

- [x] **Step 1: Write failing tests**

`test/controllers/api/v1/departments_test.rb`:

```ruby
require "test_helper"

class Api::V1::DepartmentsTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @invoices = Department.create!(name: "Invoices", parent: @finance)
    @hr = Department.create!(name: "HR")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance
  end

  test "list returns only accessible departments (subtree included)" do
    get "/api/v1/departments", headers: bearer_header("erp-app")
    assert_response :success
    names = JSON.parse(response.body).map { |d| d["name"] }
    assert_equal ["Finance", "Invoices"], names.sort
  end

  test "create a subfolder under an accessible department" do
    post "/api/v1/departments", headers: bearer_header("erp-app"),
         params: { name: "2026", parent_id: @invoices.id }
    assert_response :created
    body = JSON.parse(response.body)
    assert_equal "2026", body["name"]
    assert_equal @invoices.id, body["parent_id"]
  end

  test "create under a forbidden parent returns 403" do
    post "/api/v1/departments", headers: bearer_header("erp-app"),
         params: { name: "Payroll", parent_id: @hr.id }
    assert_response :forbidden
  end

  test "create without parent_id returns 400 (top-level is admin-only)" do
    post "/api/v1/departments", headers: bearer_header("erp-app"),
         params: { name: "Rogue" }
    assert_response :bad_request
  end

  test "duplicate sibling name returns 422" do
    post "/api/v1/departments", headers: bearer_header("erp-app"),
         params: { name: "Invoices", parent_id: @finance.id }
    assert_response :unprocessable_entity
  end
end
```

`test/controllers/api/v1/tags_test.rb`:

```ruby
require "test_helper"

class Api::V1::TagsTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance
  end

  test "create and list tags" do
    post "/api/v1/tags", headers: bearer_header("erp-app"), params: { name: "Invoice" }
    assert_response :created
    assert_equal "invoice", JSON.parse(response.body)["name"]

    get "/api/v1/tags", headers: bearer_header("erp-app")
    assert_equal ["invoice"], JSON.parse(response.body).map { |t| t["name"] }
  end

  test "duplicate tag returns 422" do
    Tag.create!(name: "invoice")
    post "/api/v1/tags", headers: bearer_header("erp-app"), params: { name: "INVOICE" }
    assert_response :unprocessable_entity
  end

  test "assign an existing tag to an accessible document" do
    doc = Document.new(department: @finance, filename: "report.pdf",
                       uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!
    Tag.create!(name: "invoice")

    post "/api/v1/documents/#{doc.id}/tags", headers: bearer_header("erp-app"),
         params: { name: "invoice" }
    assert_response :created
    assert_equal ["invoice"], doc.reload.tags.pluck(:name)

    # idempotent: assigning again neither errors nor duplicates
    post "/api/v1/documents/#{doc.id}/tags", headers: bearer_header("erp-app"),
         params: { name: "invoice" }
    assert_response :created
    assert_equal 1, doc.reload.tags.count
  end

  test "assigning an unknown tag returns 404" do
    doc = Document.new(department: @finance, filename: "report.pdf",
                       uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!
    post "/api/v1/documents/#{doc.id}/tags", headers: bearer_header("erp-app"),
         params: { name: "ghost" }
    assert_response :not_found
  end

  test "assigning a tag to a forbidden document returns 403" do
    hr = Department.create!(name: "HR")
    doc = Document.new(department: hr, filename: "salaries.pdf", uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "salaries.pdf",
                    content_type: "application/pdf")
    doc.save!
    Tag.create!(name: "invoice")
    post "/api/v1/documents/#{doc.id}/tags", headers: bearer_header("erp-app"),
         params: { name: "invoice" }
    assert_response :forbidden
  end
end
```

Run: `bin/rails test test/controllers/api`
Expected: FAIL (no routes)

- [x] **Step 2: Implement controllers + routes**

`app/controllers/api/v1/departments_controller.rb`:

```ruby
module Api
  module V1
    class DepartmentsController < BaseController
      def index
        departments = Department.where(id: current_app_client.accessible_department_ids)
                                .order(:name)
        render json: departments.map { |d| department_json(d) }
      end

      def create
        parent = Department.find(params.require(:parent_id))
        authorize_department!(parent)
        department = Department.create!(name: params.require(:name), parent: parent)
        render json: department_json(department), status: :created
      end

      private

      def department_json(department)
        { id: department.id, name: department.name, parent_id: department.parent_id }
      end
    end
  end
end
```

`app/controllers/api/v1/tags_controller.rb`:

```ruby
module Api
  module V1
    class TagsController < BaseController
      def index
        render json: Tag.order(:name).map { |t| { id: t.id, name: t.name } }
      end

      def create
        tag = Tag.create!(name: params.require(:name))
        render json: { id: tag.id, name: tag.name }, status: :created
      end
    end
  end
end
```

`app/controllers/api/v1/document_tags_controller.rb`:

```ruby
module Api
  module V1
    class DocumentTagsController < BaseController
      def create
        document = Document.find(params[:document_id])
        authorize_department!(document.department)
        tag = Tag.find_by!(name: params.require(:name).strip.downcase)
        document.tags << tag unless document.tags.exists?(tag.id)
        render json: { document_id: document.id, tags: document.tags.pluck(:name) },
               status: :created
      end
    end
  end
end
```

Update the `namespace :v1` block in `config/routes.rb`:

```ruby
    namespace :v1 do
      get "ping", to: "pings#show"
      resources :documents, only: [:index, :create, :show, :destroy] do
        member { get :download }
        resources :tags, only: [:create], controller: "document_tags"
      end
      resources :departments, only: [:index, :create]
      resources :tags, only: [:index, :create]
    end
```

- [x] **Step 3: Run tests, commit**

Run: `bin/rails test`
Expected: PASS

```bash
git add -A && git commit -m "feat: departments and tags API with tag assignment"
```

---

### Task 11: Asynchronous upload (callback URL, background job)

**Files:**
- Create: migration `*_create_upload_requests.rb`, `app/models/upload_request.rb`, `app/services/staging_store.rb`, `app/jobs/store_document_job.rb`, `app/jobs/deliver_callback_job.rb`, `test/jobs/store_document_job_test.rb`, `test/jobs/deliver_callback_job_test.rb`, `test/controllers/api/v1/async_upload_test.rb`
- Modify: `app/controllers/api/v1/documents_controller.rb` (async branch in `create`), `.gitignore` (add `/storage/staging`)

**Interfaces:**
- Consumes: Task 9 `create` action, `Document`, `authorize_department!`.
- Produces:
  - `UploadRequest` (UUID id, `status` in `pending|completed|failed`, `callback_url`, `department_id`, `filename`, `content_type`, `uploaded_by`, `document_id` (uuid, nullable), `error_message`).
  - `StagingStore.write(id, io) → Pathname`, `StagingStore.path(id) → Pathname`, `StagingStore.delete(id)` — stages raw upload bytes at `storage/staging/<id>`.
  - `StoreDocumentJob.perform_later(upload_request_id)`; `DeliverCallbackJob.perform_later(upload_request_id)` (POSTs `{ request_id, status, document_id }`, retries 5x with polynomial backoff, logs on final failure).
  - `POST /api/v1/documents` with `callback_url` → 202 `{ request_id, status: "pending" }`.

- [x] **Step 1: Write failing tests**

`test/controllers/api/v1/async_upload_test.rb`:

```ruby
require "test_helper"

class Api::V1::AsyncUploadTest < ActionDispatch::IntegrationTest
  include ActiveJob::TestHelper

  setup do
    stub_jwks!
    @finance = Department.create!(name: "Finance")
    @client = AppClient.create!(client_identifier: "erp-app")
    @client.departments << @finance
    @callback_url = "https://caller.test/ged-callback"
  end

  test "upload with callback_url returns 202 and processes in background" do
    callback = stub_request(:post, @callback_url).to_return(status: 200)

    perform_enqueued_jobs do
      post "/api/v1/documents", headers: bearer_header("erp-app"),
           params: { department_id: @finance.id,
                     file: fixture_file_upload("report.pdf", "application/pdf"),
                     callback_url: @callback_url }
      assert_response :accepted
    end

    body = JSON.parse(response.body)
    request_id = body["request_id"]
    assert_equal "pending", body["status"]

    upload_request = UploadRequest.find(request_id)
    assert_equal "completed", upload_request.status
    document = Document.find(upload_request.document_id)
    assert document.file.attached?
    assert_equal "PDF!", document.file.download
    assert_not File.exist?(StagingStore.path(request_id))

    assert_requested(callback) do |req|
      payload = JSON.parse(req.body)
      payload["request_id"] == request_id &&
        payload["status"] == "completed" &&
        payload["document_id"] == upload_request.document_id
    end
  end

  test "async upload into a forbidden department returns 403 and enqueues nothing" do
    hr = Department.create!(name: "HR")
    assert_no_enqueued_jobs do
      post "/api/v1/documents", headers: bearer_header("erp-app"),
           params: { department_id: hr.id,
                     file: fixture_file_upload("report.pdf", "application/pdf"),
                     callback_url: @callback_url }
    end
    assert_response :forbidden
  end
end
```

`test/jobs/store_document_job_test.rb`:

```ruby
require "test_helper"

class StoreDocumentJobTest < ActiveJob::TestCase
  setup do
    @finance = Department.create!(name: "Finance")
    @request = UploadRequest.create!(department: @finance, filename: "report.pdf",
                                     content_type: "application/pdf",
                                     callback_url: "https://caller.test/cb",
                                     uploaded_by: "erp-app")
    StagingStore.write(@request.id, StringIO.new("PDF!"))
  end

  test "stores the document, completes the request, enqueues the callback" do
    StoreDocumentJob.perform_now(@request.id)
    @request.reload
    assert_equal "completed", @request.status
    assert Document.find(@request.document_id).file.attached?
    assert_not File.exist?(StagingStore.path(@request.id))
    assert_enqueued_with(job: DeliverCallbackJob, args: [@request.id])
  end

  test "marks the request failed on error and still enqueues the callback" do
    StagingStore.delete(@request.id) # provoke a failure: staged file missing
    StoreDocumentJob.perform_now(@request.id)
    @request.reload
    assert_equal "failed", @request.status
    assert @request.error_message.present?
    assert_nil @request.document_id
    assert_enqueued_with(job: DeliverCallbackJob, args: [@request.id])
  end
end
```

`test/jobs/deliver_callback_job_test.rb`:

```ruby
require "test_helper"

class DeliverCallbackJobTest < ActiveJob::TestCase
  setup do
    finance = Department.create!(name: "Finance")
    @request = UploadRequest.create!(department: finance, filename: "report.pdf",
                                     callback_url: "https://caller.test/cb",
                                     uploaded_by: "erp-app", status: "completed")
  end

  test "posts the payload to the callback url" do
    callback = stub_request(:post, "https://caller.test/cb").to_return(status: 200)
    DeliverCallbackJob.new.perform(@request.id)
    assert_requested(callback) do |req|
      JSON.parse(req.body)["request_id"] == @request.id
    end
  end

  test "raises DeliveryFailed on non-2xx so retry_on kicks in" do
    stub_request(:post, "https://caller.test/cb").to_return(status: 500)
    assert_raises(DeliverCallbackJob::DeliveryFailed) do
      DeliverCallbackJob.new.perform(@request.id)
    end
  end

  test "raises DeliveryFailed on connection errors" do
    stub_request(:post, "https://caller.test/cb").to_raise(Errno::ECONNREFUSED)
    assert_raises(DeliverCallbackJob::DeliveryFailed) do
      DeliverCallbackJob.new.perform(@request.id)
    end
  end
end
```

Run: `bin/rails test test/jobs test/controllers/api/v1/async_upload_test.rb`
Expected: FAIL (constants missing)

- [x] **Step 2: Migration + model + staging store**

```bash
bin/rails generate migration CreateUploadRequests
```

```ruby
class CreateUploadRequests < ActiveRecord::Migration[8.0]
  def change
    create_table :upload_requests, id: :uuid do |t|
      t.string :status, null: false, default: "pending"
      t.string :callback_url, null: false
      t.references :department, null: false, foreign_key: true
      t.string :filename, null: false
      t.string :content_type
      t.string :uploaded_by, null: false
      t.uuid :document_id
      t.string :error_message
      t.timestamps
    end
  end
end
```

`app/models/upload_request.rb`:

```ruby
class UploadRequest < ApplicationRecord
  STATUSES = %w[pending completed failed].freeze

  belongs_to :department

  validates :status, inclusion: { in: STATUSES }
  validates :callback_url, :filename, :uploaded_by, presence: true
end
```

`app/services/staging_store.rb`:

```ruby
# Temporary on-disk staging for async uploads: the HTTP request writes the raw
# bytes here, the background job reads and deletes them.
class StagingStore
  def self.root
    Rails.root.join("storage", "staging").tap { |p| FileUtils.mkdir_p(p) }
  end

  def self.write(id, io)
    io.rewind if io.respond_to?(:rewind)
    path = root.join(id.to_s)
    File.open(path, "wb") { |f| IO.copy_stream(io, f) }
    path
  end

  def self.path(id)
    root.join(id.to_s)
  end

  def self.delete(id)
    FileUtils.rm_f(path(id))
  end
end
```

Add to `.gitignore`:

```
/storage/staging
```

Run: `bin/rails db:migrate`

- [x] **Step 3: Jobs**

`app/jobs/store_document_job.rb`:

```ruby
class StoreDocumentJob < ApplicationJob
  queue_as :default

  def perform(upload_request_id)
    upload_request = UploadRequest.find(upload_request_id)
    return unless upload_request.status == "pending"

    begin
      path = StagingStore.path(upload_request.id)
      document = Document.new(
        department_id: upload_request.department_id,
        filename: upload_request.filename,
        content_type: upload_request.content_type,
        byte_size: File.size(path),
        uploaded_by: upload_request.uploaded_by
      )
      File.open(path, "rb") do |file|
        document.file.attach(io: file, filename: upload_request.filename,
                             content_type: upload_request.content_type)
        document.save!
      end
      upload_request.update!(status: "completed", document_id: document.id)
    rescue StandardError => e
      Rails.logger.error("Async upload #{upload_request.id} failed: #{e.class}: #{e.message}")
      upload_request.update!(status: "failed", error_message: e.message)
    ensure
      StagingStore.delete(upload_request.id)
      DeliverCallbackJob.perform_later(upload_request.id)
    end
  end
end
```

`app/jobs/deliver_callback_job.rb`:

```ruby
require "net/http"

class DeliverCallbackJob < ApplicationJob
  class DeliveryFailed < StandardError; end

  queue_as :default

  retry_on DeliveryFailed, wait: :polynomially_longer, attempts: 5 do |job, error|
    Rails.logger.error(
      "Callback delivery permanently failed for upload_request #{job.arguments.first}: #{error.message}"
    )
  end

  def perform(upload_request_id)
    upload_request = UploadRequest.find(upload_request_id)
    payload = {
      request_id: upload_request.id,
      status: upload_request.status,
      document_id: upload_request.document_id
    }
    response = Net::HTTP.post(URI(upload_request.callback_url), payload.to_json,
                              "Content-Type" => "application/json")
    unless response.is_a?(Net::HTTPSuccess)
      raise DeliveryFailed, "callback returned HTTP #{response.code}"
    end
  rescue Errno::ECONNREFUSED, Errno::ETIMEDOUT, SocketError, Timeout::Error, EOFError => e
    raise DeliveryFailed, "#{e.class}: #{e.message}"
  end
end
```

- [x] **Step 4: Async branch in the documents controller**

In `app/controllers/api/v1/documents_controller.rb`, replace `create` with:

```ruby
      def create
        department = Department.find(params.require(:department_id))
        authorize_department!(department)
        file = params.require(:file)

        if params[:callback_url].present?
          return create_async(department, file)
        end

        document = Document.new(
          department: department,
          filename: file.original_filename,
          content_type: file.content_type,
          byte_size: file.size,
          uploaded_by: current_app_client.client_identifier
        )
        document.file.attach(io: file.tempfile, filename: file.original_filename,
                             content_type: file.content_type)
        document.save!
        render json: document_json(document), status: :created
      end
```

And add to the private section:

```ruby
      def create_async(department, file)
        upload_request = UploadRequest.create!(
          department: department,
          callback_url: params[:callback_url],
          filename: file.original_filename,
          content_type: file.content_type,
          uploaded_by: current_app_client.client_identifier
        )
        StagingStore.write(upload_request.id, file.tempfile)
        StoreDocumentJob.perform_later(upload_request.id)
        render json: { request_id: upload_request.id, status: "pending" },
               status: :accepted
      end
```

- [x] **Step 5: Run tests, commit**

Run: `bin/rails test`
Expected: PASS

```bash
git add -A && git commit -m "feat: async document upload with callback delivery and retries"
```

> **Executor note (Task 11):** the planned tests used
> `assert_requested(stub) { block }`, which WebMock rejects ("doesn't accept
> blocks"); switched to `assert_requested(:post, url) { block }`. The
> `.gitignore` already ignores `/storage/*`, so no staging entry was needed.

---

### Task 12: Admin web UI (departments, tags, clients, documents)

**Files:**
- Create: `app/controllers/admin/departments_controller.rb`, `app/controllers/admin/tags_controller.rb`, `app/controllers/admin/app_clients_controller.rb`, `app/controllers/admin/documents_controller.rb`, views under `app/views/admin/{departments,tags,app_clients,documents}/`, `test/controllers/admin/admin_ui_test.rb`
- Modify: `config/routes.rb`, `app/controllers/admin/dashboard_controller.rb` (redirect to documents)

**Interfaces:**
- Consumes: all models from Tasks 3–6, `Admin::BaseController` from Task 2.
- Produces: session-authenticated CRUD screens. Admin uploads set `uploaded_by: "admin:<username>"`. Admins are not subject to department restrictions (they manage everything).

- [ ] **Step 1: Write failing tests**

`test/controllers/admin/admin_ui_test.rb`:

```ruby
require "test_helper"

class Admin::AdminUiTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    @admin = Admin.create!(username: "alice", password: "secret123")
    sign_in @admin
    @finance = Department.create!(name: "Finance")
  end

  test "creates a top-level department" do
    post "/admin/departments", params: { department: { name: "HR" } }
    assert_redirected_to "/admin/departments"
    assert Department.exists?(name: "HR", parent_id: nil)
  end

  test "creates a subfolder" do
    post "/admin/departments", params: { department: { name: "Invoices", parent_id: @finance.id } }
    assert Department.exists?(name: "Invoices", parent_id: @finance.id)
  end

  test "creates and deletes a tag" do
    post "/admin/tags", params: { tag: { name: "Invoice" } }
    tag = Tag.find_by(name: "invoice")
    assert tag
    delete "/admin/tags/#{tag.id}"
    assert_not Tag.exists?(tag.id)
  end

  test "registers an app client with department access" do
    post "/admin/app_clients", params: {
      app_client: { client_identifier: "erp-app", description: "ERP",
                    department_ids: [@finance.id] }
    }
    client = AppClient.find_by(client_identifier: "erp-app")
    assert_equal [@finance.id], client.department_ids
  end

  test "uploads and deletes a document" do
    post "/admin/documents", params: {
      document: { department_id: @finance.id,
                  file: fixture_file_upload("report.pdf", "application/pdf") }
    }
    doc = Document.find_by(filename: "report.pdf")
    assert doc
    assert_equal "admin:alice", doc.uploaded_by

    get "/admin/documents"
    assert_response :success
    assert_includes response.body, "report.pdf"

    delete "/admin/documents/#{doc.id}"
    assert_not Document.exists?(doc.id)
  end

  test "downloads a document" do
    doc = Document.new(department: @finance, filename: "report.pdf", uploaded_by: "seed")
    doc.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                    content_type: "application/pdf")
    doc.save!
    get "/admin/documents/#{doc.id}/download"
    assert_equal "PDF!", response.body
  end
end
```

Run: `bin/rails test test/controllers/admin/admin_ui_test.rb`
Expected: FAIL (no routes)

- [ ] **Step 2: Routes + controllers**

In `config/routes.rb`, replace the `namespace :admin` block:

```ruby
  namespace :admin do
    root to: "dashboard#index"
    resources :departments, only: [:index, :new, :create, :destroy]
    resources :tags, only: [:index, :create, :destroy]
    resources :app_clients, except: [:show]
    resources :documents, only: [:index, :new, :create, :destroy] do
      member { get :download }
    end
  end
```

`app/controllers/admin/dashboard_controller.rb`:

```ruby
class Admin::DashboardController < Admin::BaseController
  def index
    redirect_to admin_documents_path
  end
end
```

`app/controllers/admin/departments_controller.rb`:

```ruby
class Admin::DepartmentsController < Admin::BaseController
  def index
    @departments = Department.includes(:parent).order(:name)
  end

  def new
    @department = Department.new
  end

  def create
    @department = Department.new(params.require(:department).permit(:name, :parent_id))
    if @department.save
      redirect_to admin_departments_path, notice: "Department created."
    else
      flash.now[:alert] = @department.errors.full_messages.join(", ")
      render :new, status: :unprocessable_entity
    end
  end

  def destroy
    department = Department.find(params[:id])
    if department.destroy
      redirect_to admin_departments_path, notice: "Department deleted."
    else
      redirect_to admin_departments_path,
                  alert: department.errors.full_messages.join(", ")
    end
  end
end
```

`app/controllers/admin/tags_controller.rb`:

```ruby
class Admin::TagsController < Admin::BaseController
  def index
    @tags = Tag.order(:name)
    @tag = Tag.new
  end

  def create
    tag = Tag.new(params.require(:tag).permit(:name))
    if tag.save
      redirect_to admin_tags_path, notice: "Tag created."
    else
      redirect_to admin_tags_path, alert: tag.errors.full_messages.join(", ")
    end
  end

  def destroy
    Tag.find(params[:id]).destroy
    redirect_to admin_tags_path, notice: "Tag deleted."
  end
end
```

`app/controllers/admin/app_clients_controller.rb`:

```ruby
class Admin::AppClientsController < Admin::BaseController
  def index
    @app_clients = AppClient.includes(:departments).order(:client_identifier)
  end

  def new
    @app_client = AppClient.new
  end

  def edit
    @app_client = AppClient.find(params[:id])
  end

  def create
    @app_client = AppClient.new(app_client_params)
    if @app_client.save
      redirect_to admin_app_clients_path, notice: "Client registered."
    else
      flash.now[:alert] = @app_client.errors.full_messages.join(", ")
      render :new, status: :unprocessable_entity
    end
  end

  def update
    @app_client = AppClient.find(params[:id])
    if @app_client.update(app_client_params)
      redirect_to admin_app_clients_path, notice: "Client updated."
    else
      flash.now[:alert] = @app_client.errors.full_messages.join(", ")
      render :edit, status: :unprocessable_entity
    end
  end

  def destroy
    AppClient.find(params[:id]).destroy
    redirect_to admin_app_clients_path, notice: "Client deleted."
  end

  private

  def app_client_params
    params.require(:app_client)
          .permit(:client_identifier, :description, department_ids: [])
  end
end
```

`app/controllers/admin/documents_controller.rb`:

```ruby
class Admin::DocumentsController < Admin::BaseController
  def index
    @documents = Document.includes(:department, :tags).order(created_at: :desc)
    if params[:filename].present?
      @documents = @documents.where("filename ILIKE ?",
                                    "%#{Document.sanitize_sql_like(params[:filename])}%")
    end
    @documents = @documents.where(department_id: params[:department_id]) if params[:department_id].present?
  end

  def new
    @document = Document.new
  end

  def create
    file = params.require(:document)[:file]
    if file.blank?
      redirect_to new_admin_document_path, alert: "File is required." and return
    end
    document = Document.new(
      department_id: params.require(:document)[:department_id],
      filename: file.original_filename,
      content_type: file.content_type,
      byte_size: file.size,
      uploaded_by: "admin:#{current_admin.username}"
    )
    document.file.attach(io: file.tempfile, filename: file.original_filename,
                         content_type: file.content_type)
    if document.save
      redirect_to admin_documents_path, notice: "Document uploaded."
    else
      redirect_to new_admin_document_path,
                  alert: document.errors.full_messages.join(", ")
    end
  end

  def download
    document = Document.find(params[:id])
    send_data document.file.download, filename: document.filename,
              content_type: document.content_type || "application/octet-stream"
  end

  def destroy
    Document.find(params[:id]).destroy
    redirect_to admin_documents_path, notice: "Document deleted."
  end
end
```

- [ ] **Step 3: Views**

`app/views/admin/departments/index.html.erb`:

```erb
<h1>Departments</h1>
<%= link_to "New department", new_admin_department_path %>
<table>
  <tr><th>Name</th><th>Parent</th><th></th></tr>
  <% @departments.each do |dept| %>
    <tr>
      <td><%= dept.name %></td>
      <td><%= dept.parent&.name %></td>
      <td><%= button_to "Delete", admin_department_path(dept), method: :delete,
              data: { turbo_confirm: "Delete #{dept.name}?" } %></td>
    </tr>
  <% end %>
</table>
```

`app/views/admin/departments/new.html.erb`:

```erb
<h1>New department</h1>
<%= form_with model: [:admin, @department] do |f| %>
  <div><%= f.label :name %> <%= f.text_field :name %></div>
  <div><%= f.label :parent_id, "Parent (blank = top-level)" %>
    <%= f.collection_select :parent_id, Department.order(:name), :id, :name,
                            include_blank: true %></div>
  <%= f.submit "Create" %>
<% end %>
```

`app/views/admin/tags/index.html.erb`:

```erb
<h1>Tags</h1>
<%= form_with model: [:admin, @tag] do |f| %>
  <%= f.text_field :name, placeholder: "new tag" %> <%= f.submit "Add" %>
<% end %>
<ul>
  <% @tags.each do |tag| %>
    <li><%= tag.name %>
      <%= button_to "Delete", admin_tag_path(tag), method: :delete %></li>
  <% end %>
</ul>
```

`app/views/admin/app_clients/index.html.erb`:

```erb
<h1>Application clients</h1>
<%= link_to "Register client", new_admin_app_client_path %>
<table>
  <tr><th>client_id</th><th>Description</th><th>Departments</th><th></th></tr>
  <% @app_clients.each do |client| %>
    <tr>
      <td><%= client.client_identifier %></td>
      <td><%= client.description %></td>
      <td><%= client.departments.map(&:name).join(", ") %></td>
      <td>
        <%= link_to "Edit", edit_admin_app_client_path(client) %>
        <%= button_to "Delete", admin_app_client_path(client), method: :delete %>
      </td>
    </tr>
  <% end %>
</table>
```

`app/views/admin/app_clients/_form.html.erb`:

```erb
<%= form_with model: [:admin, @app_client] do |f| %>
  <div><%= f.label :client_identifier %> <%= f.text_field :client_identifier %></div>
  <div><%= f.label :description %> <%= f.text_field :description %></div>
  <fieldset>
    <legend>Department access</legend>
    <%= f.collection_check_boxes :department_ids, Department.order(:name), :id, :name do |b| %>
      <div><%= b.checkbox %> <%= b.label %></div>
    <% end %>
  </fieldset>
  <%= f.submit %>
<% end %>
```

`app/views/admin/app_clients/new.html.erb`:

```erb
<h1>Register client</h1>
<%= render "form" %>
```

`app/views/admin/app_clients/edit.html.erb`:

```erb
<h1>Edit client</h1>
<%= render "form" %>
```

`app/views/admin/documents/index.html.erb`:

```erb
<h1>Documents</h1>
<%= link_to "Upload document", new_admin_document_path %>
<%= form_with url: admin_documents_path, method: :get do |f| %>
  <%= f.text_field :filename, value: params[:filename], placeholder: "filename contains…" %>
  <%= f.select :department_id,
        options_from_collection_for_select(Department.order(:name), :id, :name,
                                           params[:department_id]),
        include_blank: "all departments" %>
  <%= f.submit "Filter" %>
<% end %>
<table>
  <tr><th>Filename</th><th>Department</th><th>Tags</th><th>Uploaded by</th><th>Date</th><th></th></tr>
  <% @documents.each do |doc| %>
    <tr>
      <td><%= doc.filename %></td>
      <td><%= doc.department.name %></td>
      <td><%= doc.tags.map(&:name).join(", ") %></td>
      <td><%= doc.uploaded_by %></td>
      <td><%= doc.created_at.strftime("%Y-%m-%d %H:%M") %></td>
      <td>
        <%= link_to "Download", download_admin_document_path(doc) %>
        <%= button_to "Delete", admin_document_path(doc), method: :delete,
            data: { turbo_confirm: "Delete #{doc.filename}?" } %>
      </td>
    </tr>
  <% end %>
</table>
```

`app/views/admin/documents/new.html.erb`:

```erb
<h1>Upload document</h1>
<%= form_with model: [:admin, @document] do |f| %>
  <div><%= f.label :department_id %>
    <%= f.collection_select :department_id, Department.order(:name), :id, :name %></div>
  <div><%= f.label :file %> <%= f.file_field :file %></div>
  <%= f.submit "Upload" %>
<% end %>
```

- [ ] **Step 4: Run tests, commit**

Run: `bin/rails test`
Expected: PASS

```bash
git add -A && git commit -m "feat: admin UI for departments, tags, clients, documents"
```

---

### Task 13: oauth-provider — standalone Sinatra OAuth2 stopgap

A self-contained app in `oauth-provider/` with its own Gemfile. It issues RS256 client_credentials JWTs and exposes JWKS + OIDC discovery, mimicking the future company IdP. SQLite stores clients (secret stored as SHA-256 digest — acceptable for this dev-only stopgap; documented in its README).

**Files:**
- Create: `oauth-provider/Gemfile`, `oauth-provider/config.ru`, `oauth-provider/app.rb`, `oauth-provider/lib/key_store.rb`, `oauth-provider/lib/client_store.rb`, `oauth-provider/bin/register-client`, `oauth-provider/test/app_test.rb`, `oauth-provider/Rakefile`, `oauth-provider/README.md`, `oauth-provider/.gitignore`

**Interfaces:**
- Produces (HTTP, consumed by the GED `web` service and by test clients):
  - `POST /token` — form params `grant_type=client_credentials`, `client_id`, `client_secret` → `{ access_token, token_type: "Bearer", expires_in: 3600 }`; the JWT has claims `iss` (env `OAUTH_ISSUER`), `sub` (= client_id), `iat`, `exp`, `jti`, header `kid`.
  - `GET /.well-known/jwks.json` → `{ keys: [public JWK] }`
  - `GET /.well-known/openid-configuration` → `{ issuer, token_endpoint, jwks_uri }`
  - CLI: `bin/register-client <client_id>` prints a generated secret once.

- [ ] **Step 1: Project skeleton and failing tests**

`oauth-provider/Gemfile`:

```ruby
source "https://rubygems.org"

gem "sinatra", "~> 4.1"
gem "puma"
gem "rackup"
gem "jwt"
gem "sqlite3"

group :test do
  gem "minitest"
  gem "rack-test"
  gem "rake"
end
```

`oauth-provider/.gitignore`:

```
data/
```

`oauth-provider/Rakefile`:

```ruby
require "rake/testtask"

Rake::TestTask.new do |t|
  t.libs << "test"
  t.pattern = "test/**/*_test.rb"
end

task default: :test
```

`oauth-provider/test/app_test.rb`:

```ruby
ENV["OAUTH_ISSUER"] = "http://oauth-provider.test"
ENV["OAUTH_DATA_DIR"] = File.expand_path("../tmp_test_data", __dir__)

require "fileutils"
FileUtils.rm_rf(ENV["OAUTH_DATA_DIR"])

require "minitest/autorun"
require "rack/test"
require_relative "../app"

class AppTest < Minitest::Test
  include Rack::Test::Methods

  def app
    OauthProvider::App
  end

  def setup
    @secret = OauthProvider::ClientStore.register("erp-app")
  end

  def teardown
    OauthProvider::ClientStore.reset_for_tests!
  end

  def test_issues_a_valid_jwt_for_correct_credentials
    post "/token", grant_type: "client_credentials",
                   client_id: "erp-app", client_secret: @secret
    assert_equal 200, last_response.status
    body = JSON.parse(last_response.body)
    assert_equal "Bearer", body["token_type"]
    assert_equal 3600, body["expires_in"]

    jwks = JWT::JWK::Set.new(OauthProvider::KeyStore.jwks)
    payload, header = JWT.decode(body["access_token"], nil, true,
                                 algorithms: ["RS256"], jwks: jwks)
    assert_equal "erp-app", payload["sub"]
    assert_equal "http://oauth-provider.test", payload["iss"]
    assert payload["exp"] > Time.now.to_i
    assert header["kid"]
  end

  def test_rejects_wrong_secret
    post "/token", grant_type: "client_credentials",
                   client_id: "erp-app", client_secret: "wrong"
    assert_equal 401, last_response.status
  end

  def test_rejects_unknown_client
    post "/token", grant_type: "client_credentials",
                   client_id: "ghost", client_secret: "whatever"
    assert_equal 401, last_response.status
  end

  def test_rejects_unsupported_grant_type
    post "/token", grant_type: "password",
                   client_id: "erp-app", client_secret: @secret
    assert_equal 400, last_response.status
  end

  def test_serves_jwks
    get "/.well-known/jwks.json"
    assert_equal 200, last_response.status
    keys = JSON.parse(last_response.body)["keys"]
    assert_equal 1, keys.size
    assert_equal "RS256", keys.first["alg"]
    refute keys.first.key?("d"), "JWKS must not expose the private key"
  end

  def test_serves_openid_configuration
    get "/.well-known/openid-configuration"
    body = JSON.parse(last_response.body)
    assert_equal "http://oauth-provider.test", body["issuer"]
    assert body["token_endpoint"].end_with?("/token")
    assert body["jwks_uri"].end_with?("/.well-known/jwks.json")
  end
end
```

Run:

```bash
cd oauth-provider && bundle install && bundle exec rake
```

Expected: FAIL (app.rb missing)

- [ ] **Step 2: Implement key store, client store, app**

`oauth-provider/lib/key_store.rb`:

```ruby
require "openssl"
require "jwt"
require "fileutils"

module OauthProvider
  # Generates an RSA signing key on first boot and persists it so tokens
  # survive restarts. kid is derived from the JWK thumbprint.
  module KeyStore
    module_function

    def data_dir
      dir = ENV.fetch("OAUTH_DATA_DIR", File.expand_path("../data", __dir__))
      FileUtils.mkdir_p(dir)
      dir
    end

    def signing_key
      @signing_key ||= begin
        path = File.join(data_dir, "signing_key.pem")
        if File.exist?(path)
          OpenSSL::PKey::RSA.new(File.read(path))
        else
          key = OpenSSL::PKey::RSA.new(2048)
          File.write(path, key.to_pem)
          File.chmod(0o600, path)
          key
        end
      end
    end

    def jwk
      @jwk ||= JWT::JWK.new(signing_key, { use: "sig", alg: "RS256" })
    end

    def jwks
      { keys: [jwk.export] } # export without private key
    end
  end
end
```

`oauth-provider/lib/client_store.rb`:

```ruby
require "sqlite3"
require "securerandom"
require "digest"

module OauthProvider
  # SQLite-backed client registry. Secrets are stored as SHA-256 digests —
  # fine for a dev-only stopgap, NOT a production credential store.
  module ClientStore
    module_function

    def db
      @db ||= begin
        db = SQLite3::Database.new(File.join(KeyStore.data_dir, "clients.sqlite3"))
        db.execute <<~SQL
          CREATE TABLE IF NOT EXISTS clients (
            client_id TEXT PRIMARY KEY,
            secret_digest TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
          )
        SQL
        db
      end
    end

    def register(client_id)
      secret = SecureRandom.hex(32)
      db.execute("INSERT OR REPLACE INTO clients (client_id, secret_digest) VALUES (?, ?)",
                 [client_id, Digest::SHA256.hexdigest(secret)])
      secret
    end

    def authenticate(client_id, client_secret)
      return false if client_id.to_s.empty? || client_secret.to_s.empty?
      row = db.get_first_row("SELECT secret_digest FROM clients WHERE client_id = ?", [client_id])
      return false unless row
      expected = row[0]
      actual = Digest::SHA256.hexdigest(client_secret)
      OpenSSL.secure_compare(expected, actual)
    end

    def reset_for_tests!
      db.execute("DELETE FROM clients")
    end
  end
end
```

`oauth-provider/app.rb`:

```ruby
require "sinatra/base"
require "json"
require "securerandom"
require_relative "lib/key_store"
require_relative "lib/client_store"

module OauthProvider
  class App < Sinatra::Base
    TOKEN_TTL = 3600

    def issuer
      ENV.fetch("OAUTH_ISSUER", "http://localhost:9292")
    end

    post "/token" do
      content_type :json
      unless params["grant_type"] == "client_credentials"
        halt 400, { error: "unsupported_grant_type" }.to_json
      end
      unless ClientStore.authenticate(params["client_id"], params["client_secret"])
        halt 401, { error: "invalid_client" }.to_json
      end

      now = Time.now.to_i
      payload = {
        iss: issuer,
        sub: params["client_id"],
        iat: now,
        exp: now + TOKEN_TTL,
        jti: SecureRandom.uuid
      }
      token = JWT.encode(payload, KeyStore.signing_key, "RS256", kid: KeyStore.jwk.kid)
      { access_token: token, token_type: "Bearer", expires_in: TOKEN_TTL }.to_json
    end

    get "/.well-known/jwks.json" do
      content_type :json
      KeyStore.jwks.to_json
    end

    get "/.well-known/openid-configuration" do
      content_type :json
      {
        issuer: issuer,
        token_endpoint: "#{issuer}/token",
        jwks_uri: "#{issuer}/.well-known/jwks.json"
      }.to_json
    end
  end
end
```

`oauth-provider/config.ru`:

```ruby
require_relative "app"
run OauthProvider::App
```

`oauth-provider/bin/register-client` (make executable with `chmod +x`):

```ruby
#!/usr/bin/env ruby
require_relative "../lib/key_store"
require_relative "../lib/client_store"

client_id = ARGV[0]
abort "usage: bin/register-client <client_id>" if client_id.to_s.empty?

secret = OauthProvider::ClientStore.register(client_id)
puts "client_id:     #{client_id}"
puts "client_secret: #{secret}"
puts "(store this secret now — it is not retrievable later)"
```

`oauth-provider/README.md`:

```markdown
# oauth-provider — dev/test OAuth2 stopgap

Stands in for the company IdP until it is accessible. Issues RS256
client_credentials JWTs and exposes JWKS + OIDC discovery. SQLite storage,
secrets stored as SHA-256 digests — dev-only, not a production credential store.

## Usage

    bundle install
    bundle exec rackup -p 9292          # start on :9292
    bin/register-client my-app          # prints client_id + secret (once)

    # get a token
    curl -s -X POST http://localhost:9292/token \
      -d grant_type=client_credentials -d client_id=my-app -d client_secret=<secret>

Cutover to the real IdP = change `GED_OIDC_ISSUER` / `GED_OIDC_JWKS_URL` on the
GED web service, then delete this app.
```

- [ ] **Step 3: Run tests**

Run: `cd oauth-provider && bundle exec rake`
Expected: PASS (6 tests)

- [ ] **Step 4: End-to-end smoke test against the GED app (manual)**

```bash
# terminal A
cd oauth-provider && bundle exec rackup -p 9292
# terminal B — run GED against the local provider
cd /Users/fourreto/Projects/vibecode/ged
oauth-provider/bin/register-client smoke-app   # note the secret
GED_OIDC_ISSUER=http://localhost:9292 \
GED_OIDC_JWKS_URL=http://localhost:9292/.well-known/jwks.json \
bin/rails server
# terminal C
TOKEN=$(curl -s -X POST http://localhost:9292/token -d grant_type=client_credentials \
  -d client_id=smoke-app -d client_secret=<secret> | ruby -rjson -e 'puts JSON.parse(STDIN.read)["access_token"]')
curl -s http://localhost:3000/api/v1/ping -H "Authorization: Bearer $TOKEN"
# expect 403 {"error":"client not registered"} — token valid, client not yet mapped.
# Register "smoke-app" + a department in the admin UI (http://localhost:3000, admin/changeme
# after bin/rails db:seed), grant access, re-run: expect {"client":"smoke-app"}
```

Expected: the 401→403→200 progression proves JWKS validation and provisioning work end-to-end.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: standalone Sinatra OAuth2 dev provider (client_credentials + JWKS)"
```

---

### Task 14: Docker Compose deployment + docs

**Prerequisite (human/manual):** Docker is NOT installed on this machine. Install OrbStack (`brew install --cask orbstack`) or Docker Desktop before the verification steps. If Docker cannot be installed in this session, complete the files, verify `docker compose config` is not runnable, note it in the commit, and leave runtime verification as a documented manual step.

**Files:**
- Create: `oauth-provider/Dockerfile`, `docker-compose.yml`, `README.md` (repo root)
- Modify: `config/environments/production.rb` (disable force_ssl for LAN http), `Dockerfile` (generated by rails new — verify, no change expected), `CLAUDE.md` (fill Conventions section)

**Interfaces:**
- Consumes: everything built in Tasks 1–13.
- Produces: `docker compose up` brings up the full stack; documented bootstrap procedure.

- [ ] **Step 1: production config tweaks**

In `config/environments/production.rb`, set (the app runs on a LAN over plain HTTP by design — see spec Non-goals):

```ruby
  config.force_ssl = false
  config.assume_ssl = false
```

Also ensure Active Storage stays on local disk in production (`config.active_storage.service = :local` — this is the generated default; verify).

- [ ] **Step 2: oauth-provider Dockerfile**

`oauth-provider/Dockerfile`:

```dockerfile
FROM ruby:3.4-slim

RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends build-essential libsqlite3-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Gemfile Gemfile.lock ./
RUN bundle install
COPY . .

EXPOSE 9292
CMD ["bundle", "exec", "rackup", "--host", "0.0.0.0", "-p", "9292"]
```

(Generate `Gemfile.lock` first if missing: `cd oauth-provider && bundle lock`.)

- [ ] **Step 3: docker-compose.yml at repo root**

```yaml
services:
  postgres:
    image: postgres:17
    environment:
      POSTGRES_PASSWORD: postgres
    volumes:
      - pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 5s
      timeout: 3s
      retries: 10

  oauth-provider:
    build: ./oauth-provider
    environment:
      OAUTH_ISSUER: http://oauth-provider:9292
    ports:
      - "9292:9292"
    volumes:
      - oauth_data:/app/data

  web:
    build: .
    depends_on:
      postgres:
        condition: service_healthy
      oauth-provider:
        condition: service_started
    environment:
      DATABASE_URL: postgres://postgres:postgres@postgres:5432/ged_production
      SECRET_KEY_BASE: ${SECRET_KEY_BASE:?run: SECRET_KEY_BASE=$(bin/rails secret) docker compose up}
      RAILS_ENV: production
      SOLID_QUEUE_IN_PUMA: "true"
      GED_OIDC_ISSUER: http://oauth-provider:9292
      GED_OIDC_JWKS_URL: http://oauth-provider:9292/.well-known/jwks.json
      GED_ADMIN_USERNAME: ${GED_ADMIN_USERNAME:-admin}
      GED_ADMIN_PASSWORD: ${GED_ADMIN_PASSWORD:?set GED_ADMIN_PASSWORD}
    ports:
      - "3000:3000"
    volumes:
      - storage:/rails/storage

volumes:
  pgdata:
  oauth_data:
  storage:
```

Check the generated Rails `Dockerfile` and `bin/docker-entrypoint`: the entrypoint must run `./bin/rails db:prepare` (Rails 8 generates this). Rails 8's production `config/database.yml` defines `primary`, `cache`, `queue` databases — `db:prepare` creates all three; `DATABASE_URL` overrides the primary. Verify `config/database.yml` production section uses distinct `database:` names with `url: <%= ENV["DATABASE_URL"] %>` on primary only; adjust so cache/queue get their own DATABASE names on the same server, e.g.:

```yaml
production:
  primary: &primary_production
    <<: *default
    url: <%= ENV["DATABASE_URL"] %>
  cache:
    <<: *primary_production
    url: <%= ENV["DATABASE_URL"]&.sub("ged_production", "ged_production_cache") %>
    migrations_paths: db/cache_migrate
  queue:
    <<: *primary_production
    url: <%= ENV["DATABASE_URL"]&.sub("ged_production", "ged_production_queue") %>
    migrations_paths: db/queue_migrate
```

- [ ] **Step 4: Root README**

`README.md`:

```markdown
# GED — document management system

Rails app + PostgreSQL + temporary OAuth2 dev provider. Spec:
`docs/superpowers/specs/2026-07-08-ged-design.md`.

## Development

    brew install mise postgresql@17 && brew services start postgresql@17
    mise install                # ruby from .mise.toml
    bundle install
    bin/rails db:setup          # create + migrate + seed (admin/changeme)
    bin/rails test              # main app tests
    (cd oauth-provider && bundle install && bundle exec rake)  # provider tests
    bin/rails server            # http://localhost:3000 → admin UI

## Deployment (Docker Compose)

    SECRET_KEY_BASE=$(bin/rails secret) GED_ADMIN_PASSWORD=<pw> docker compose up --build -d
    docker compose exec web bin/rails db:seed     # first boot only: create admin

## Bootstrap an application client

1. `docker compose exec oauth-provider bin/register-client my-app` → note the secret
2. Admin UI (http://localhost:3000) → Clients → Register `my-app` + department access
3. `curl -X POST http://localhost:9292/token -d grant_type=client_credentials -d client_id=my-app -d client_secret=<secret>`
4. Call the API: `curl http://localhost:3000/api/v1/documents -H "Authorization: Bearer <token>"`

NOTE: containers reach the provider at `http://oauth-provider:9292` (compose DNS);
tokens fetched from the host at `localhost:9292` carry `iss=http://oauth-provider:9292`
(set via OAUTH_ISSUER), so they validate correctly inside the stack.

## Verification checklist (manual, after deploy)

- [ ] Admin login works; create departments Finance + HR
- [ ] Register client, grant Finance only
- [ ] Upload to Finance → 201 + uuid; upload to HR → 403
- [ ] List/download/delete document; tag a document; filter by tag
- [ ] Async upload with callback_url (use https://webhook.site) → 202 then callback with document_id
- [ ] Restart stack (`docker compose restart`) → documents still present
```

- [ ] **Step 5: Fill CLAUDE.md Conventions section**

Replace the placeholder `## Conventions` section in `CLAUDE.md` with:

```markdown
## Conventions

- Tests: Minitest (Rails default), WebMock for HTTP stubs. Run `bin/rails test`
  (main app) and `cd oauth-provider && bundle exec rake` (provider).
- Lint: RuboCop rails-omakase — `bin/rubocop` must pass before committing.
- Ruby via mise (`.mise.toml`); dev Postgres via Homebrew (`postgresql@17`).
- Env vars: `GED_OIDC_ISSUER`, `GED_OIDC_JWKS_URL`, `GED_CLIENT_ID_CLAIM`
  (default `sub`), `GED_ADMIN_USERNAME`, `GED_ADMIN_PASSWORD`.
- API error contract: 401 invalid/missing token, 403 unregistered client or
  forbidden department, 404 not found, 400 missing param, 422 validation.
```

- [ ] **Step 6: Verify and commit**

If Docker is available:

```bash
SECRET_KEY_BASE=$(bin/rails secret) GED_ADMIN_PASSWORD=changeme docker compose up --build -d
curl -s http://localhost:3000/up          # expect 200
curl -s http://localhost:9292/.well-known/jwks.json | head -c 200
docker compose down
```

Always:

```bash
bin/rails test && bin/rubocop
git add -A && git commit -m "feat: docker compose deployment, README, conventions"
```

---

## Plan self-review notes

- **Spec coverage:** departments/hierarchy/subtree permissions (T3, T6, T8–9), tags (T4, T10), documents + Active Storage (T5, T9), app clients + manual provisioning (T6, T12), JWKS resource-server auth fail-closed (T7–8), admin Devise UI (T2, T12), sync/async upload + callback retries (T9, T11), local OAuth provider (T13), Docker Compose (T14). Non-goals respected: no full-text search, no versioning, no backups, no Kafka, no TLS.
- **Known deviation from spec (approved during planning):** the oauth-provider is hand-rolled Sinatra + `jwt`, not Doorkeeper (Doorkeeper cannot issue JWT client_credentials tokens without extra gems and a full Rails app; spec updated accordingly).
- **Environment quirk:** no Docker on the dev machine — Task 14 degrades gracefully to file-only + documented manual verification.
