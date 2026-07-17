require "application_system_test_case"

# Automates section 2 ("Interface d'administration") of docs/qa-test-plan.md
# against the real admin UI, driven through a headless Chrome browser.
class AdminUiTest < ApplicationSystemTestCase
  setup do
    # find_or_initialize: bin/ci's seeds step leaves a permanent "admin" row
    # in the test database, which a plain create! would collide with
    @admin = Admin.find_or_initialize_by(username: "admin")
    @admin.update!(password: "changeme")
  end

  def report_pdf_path
    Rails.root.join("test/fixtures/files/report.pdf")
  end

  # --- 2.1 / 2.2 Login -----------------------------------------------------

  test "2.1 login with valid credentials lands on the documents list" do
    sign_in_admin
    assert_current_path admin_documents_path
    assert_selector "h1", text: "Documents"
    assert_button "Log out"
  end

  test "2.2 login with a wrong password is rejected and creates no session" do
    visit "/admin/documents"
    fill_in "Username", with: "admin"
    fill_in "Password", with: "wrong-password"
    click_button "Log in"

    assert_no_button "Log out"
    assert_selector "h2", text: "Log in"
    # And a protected page still bounces us back to login.
    visit "/admin/documents"
    assert_selector "h2", text: "Log in"
  end

  # --- 2.3 / 2.4 Departments ----------------------------------------------

  test "2.3 create departments with a hierarchy" do
    sign_in_admin

    create_department "Finance"
    create_department "HR"
    create_department "Invoices", parent: "Finance"

    within "table" do
      assert_selector "td", text: "Finance"
      assert_selector "td", text: "HR"
      # Invoices row shows Finance as its parent.
      assert_selector "tr", text: "Invoices Finance"
    end
  end

  test "2.4 a duplicate department at the same level is rejected" do
    sign_in_admin
    create_department "Finance"
    assert_selector "td", text: "Finance" # first one landed on the index

    create_department "Finance", expect: :failure # duplicate at the same (top) level
    assert_text "has already been taken"
    assert_equal 1, Department.where(name: "Finance", parent_id: nil).count
  end

  # --- 2.5 Tags ------------------------------------------------------------

  test "2.5 tag keys are normalized to lowercase in the document tags textarea" do
    finance = Department.create!(name: "Finance")
    attach_report(finance, filename: "report.pdf")
    sign_in_admin

    visit admin_documents_path
    within("tr", text: "report.pdf") { click_link "Tags" }
    fill_in "document[tags_text]", with: "Type=Facture\nUrgent"
    click_button "Save tags"

    assert_text "Tags updated."
    # Tags render as key/value chips: keys are downcased, values keep their
    # case, and a bare key (no value) shows as a solo chip.
    assert_selector ".tag .k", text: "type"
    assert_selector ".tag .v", text: "Facture"
    assert_selector ".tag.solo .k", text: "urgent"
  end

  # --- 2.6 / 2.7 / 2.8 / 2.9 Documents -------------------------------------

  test "2.6 upload a document and see it in the list attributed to the admin" do
    Department.create!(name: "Finance")
    sign_in_admin

    visit new_admin_document_path
    select "Finance", from: "Department"
    attach_file "File", report_pdf_path
    click_button "Upload"

    assert_text "Document uploaded."
    within "table" do
      assert_selector "tr", text: "report.pdf"
      assert_selector "tr", text: "admin:admin"
    end
  end

  test "2.7 download the uploaded document returns the original bytes" do
    upload_report_into("Finance")
    document = Document.find_by!(filename: "report.pdf")

    # Fetch the download endpoint directly (browser downloads are awkward to
    # assert on); the session cookie is shared with the browser.
    body = download_via_session(download_admin_document_path(document))
    assert_equal File.binread(report_pdf_path), body
  end

  test "2.8 filter the document list by filename and by department" do
    finance = Department.create!(name: "Finance")
    hr = Department.create!(name: "HR")
    attach_report(finance, filename: "invoice.pdf")
    attach_report(hr, filename: "handbook.pdf")
    sign_in_admin

    visit admin_documents_path
    assert_selector "tr", text: "invoice.pdf"
    assert_selector "tr", text: "handbook.pdf"

    # Filter by filename.
    fill_in "filename", with: "invoice"
    click_button "Filter"
    assert_selector "tr", text: "invoice.pdf"
    assert_no_selector "tr", text: "handbook.pdf"

    # Filter by department — start from a fresh page so the reselect doesn't
    # race the previous GET reload's DOM swap.
    visit admin_documents_path
    select "HR", from: "department_id"
    click_button "Filter"
    assert_selector "tr", text: "handbook.pdf"
    assert_no_selector "tr", text: "invoice.pdf"
  end

  test "2.9 delete a document removes it from the list" do
    upload_report_into("Finance")
    visit admin_documents_path
    assert_selector "tr", text: "report.pdf"

    # The admin layout ships no JavaScript, so the turbo-confirm never fires;
    # the delete button just submits its form.
    within("tr", text: "report.pdf") { click_button "Delete" }

    assert_text "Document moved to trash."
    assert_no_selector "tr", text: "report.pdf"
  end

  test "2.9b a deleted document can be restored from the trash" do
    upload_report_into("Finance")
    visit admin_documents_path
    within("tr", text: "report.pdf") { click_button "Delete" }
    assert_text "Document moved to trash."
    assert_no_selector "tr", text: "report.pdf"

    click_link "Trash"
    assert_selector "h1", text: "Trash"
    assert_selector "tr", text: "report.pdf"

    within("tr", text: "report.pdf") { click_button "Restore" }
    assert_text "Document restored."

    click_link "Documents"
    assert_selector "tr", text: "report.pdf"
  end

  # --- 2.10 Deleting a non-empty department opens the resolution flow ------

  test "2.10 deleting a department with documents offers move or trash, then trashes" do
    finance = Department.create!(name: "Finance")
    Department.create!(name: "Archives")
    attach_report(finance, filename: "report.pdf")
    sign_in_admin

    # Deleting a non-empty department is intercepted by the resolution page,
    # which offers both moving the documents elsewhere and trashing everything.
    visit admin_departments_path
    within("tr", text: "Finance") { click_button "Delete" }

    assert_text "This department holds 1 active and 0 trashed document(s)."
    assert_button "Move documents and delete department"
    assert_button "Trash department and its documents"
    assert_equal 1, Department.where(name: "Finance").count, "must not delete before resolving"

    # Trash everything: the department is soft-deleted, greyed in the list,
    # and offers a Restore action instead of Rename/Delete.
    click_button "Trash department and its documents"

    assert_selector "tr.trashed", text: "Finance"
    within("tr", text: "Finance") { assert_button "Restore" }
    assert Department.with_deleted.find_by(name: "Finance").deleted_at,
           "department should be soft-deleted"

    # Restore brings the department (and its documents) back to life.
    within("tr.trashed", text: "Finance") { click_button "Restore" }

    assert_no_selector "tr.trashed", text: "Finance"
    assert_nil Department.find_by(name: "Finance").deleted_at
  end

  # --- 2.11 Logout ---------------------------------------------------------

  test "2.11 after logout a protected page redirects to login" do
    sign_in_admin
    click_button "Log out"

    visit "/admin/documents"
    assert_selector "h2", text: "Log in"
    assert_no_button "Log out"
  end

  # --- Status page ---------------------------------------------------------

  test "the status page shows health checks and key metrics" do
    sign_in_admin
    click_link "Status"

    assert_selector "h1", text: "Status"
    assert_selector ".health-row", count: 4
    assert_text "Database"
    assert_text "Storage disk"
    assert_selector ".stat", minimum: 5
  end

  # --- Theme toggle --------------------------------------------------------

  test "the light/dark toggle switches the theme and persists across reloads" do
    sign_in_admin

    initial = page.evaluate_script("document.documentElement.dataset.theme")
    toggled = initial == "dark" ? "light" : "dark"

    find("#theme-toggle").click
    assert_equal toggled, page.evaluate_script("document.documentElement.dataset.theme")

    # The choice is stored and re-applied before paint on the next page load.
    visit admin_documents_path
    assert_equal toggled, page.evaluate_script("document.documentElement.dataset.theme")
  end

  private

  def create_department(name, parent: nil, expect: :success)
    visit new_admin_department_path
    fill_in "Name", with: name
    select parent, from: "Parent (blank = top-level)" if parent
    click_button "Create"
    # Wait for the redirect+flash to settle before the caller moves on, so a
    # following `visit` doesn't race the in-flight submission.
    assert_text "Department created." if expect == :success
  end

  def upload_report_into(department_name)
    Department.find_or_create_by!(name: department_name)
    sign_in_admin
    visit new_admin_document_path
    select department_name, from: "Department"
    attach_file "File", report_pdf_path
    click_button "Upload"
    assert_text "Document uploaded."
  end

  # Build a Document straight from the model for scenarios where the upload UI
  # itself isn't under test (filtering, delete, dependent-restrict).
  def attach_report(department, filename: "report.pdf")
    document = Document.new(
      department: department,
      filename: filename,
      content_type: "application/pdf",
      byte_size: File.size(report_pdf_path),
      uploaded_by: "admin:admin"
    )
    document.file.attach(io: File.open(report_pdf_path), filename: filename,
                         content_type: "application/pdf")
    document.save!
    document
  end

  # Reuse the browser's session cookie to fetch a URL over plain HTTP, since
  # asserting on a real browser download in headless Chrome is unwieldy.
  def download_via_session(path)
    cookie = page.driver.browser.manage.cookie_named("_ged_session")
    base = Capybara.current_session.server.base_url
    uri = URI.join(base, path)
    request = Net::HTTP::Get.new(uri)
    request["Cookie"] = "#{cookie[:name]}=#{cookie[:value]}"
    Net::HTTP.start(uri.host, uri.port) { |http| http.request(request) }.body
  end
end
