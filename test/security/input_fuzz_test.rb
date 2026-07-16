require "test_helper"

# White-box regression suite from the 2026-07-15 pentest pass
# (docs/security/pentest-2026-07-15.md). Locks in input-hardening defenses
# (nested/array param confusion, malformed filters, pagination abuse, tag
# limits) and documents the confirmed-but-unfixed gaps (GED-SEC-02/03/05) as
# skips so they can be un-skipped once fixed.
class InputFuzzTest < ActionDispatch::IntegrationTest
  setup do
    stub_jwks!
    @department = Department.create!(name: "Fuzz")
    @client = AppClient.create!(client_identifier: "deptA")
    @client.client_department_accesses.create!(
      department: @department, can_read: true, can_write: true
    )
    @document = Document.new(department: @department, filename: "report.pdf",
                             content_type: "application/pdf", byte_size: 4,
                             uploaded_by: "seed")
    @document.file.attach(io: StringIO.new("PDF!"), filename: "report.pdf",
                          content_type: "application/pdf")
    @document.save!
  end

  def upload_file
    fixture_file_upload("report.pdf", "application/pdf")
  end

  # --- non-scalar param confusion ---

  test "a nested hash where a scalar param is expected returns 400, not 500" do
    get "/api/v1/documents", headers: bearer_header("deptA"),
        params: { department_id: { a: "b" } }
    assert_response :bad_request
  end

  test "an array where a scalar param is expected returns 400, not 500" do
    get "/api/v1/documents", headers: bearer_header("deptA"),
        params: { filename: [ "x" ] }
    assert_response :bad_request
  end

  # --- malformed filter values ---

  test "tag_value without tag returns 400" do
    get "/api/v1/documents", headers: bearer_header("deptA"),
        params: { tag_value: "x" }
    assert_response :bad_request
  end

  test "an unparsable date filter returns 400" do
    get "/api/v1/documents", headers: bearer_header("deptA"),
        params: { uploaded_after: "not-a-date" }
    assert_response :bad_request
  end

  test "an invalid upload_mode returns 400" do
    get "/api/v1/documents", headers: bearer_header("deptA"),
        params: { upload_mode: "bogus" }
    assert_response :bad_request
  end

  # --- pagination abuse ---

  test "a negative limit is clamped, not a 500" do
    get "/api/v1/documents", headers: bearer_header("deptA"), params: { limit: -5 }
    assert_response :success
  end

  test "a huge limit is clamped, not a 500" do
    get "/api/v1/documents", headers: bearer_header("deptA"), params: { limit: 9_999_999 }
    assert_response :success
  end

  test "a negative offset is clamped, not a 500" do
    get "/api/v1/documents", headers: bearer_header("deptA"), params: { offset: -1 }
    assert_response :success
  end

  test "a pagination offset beyond bigint range is handled, not a 500" do
    # Desired behavior: offset should be capped/clamped like limit already
    # is, so it never reaches Postgres as an out-of-range bigint. Currently
    # `paginate` clamps limit to MAX_PAGE_SIZE but passes offset straight
    # through to `.offset(...)` with no upper bound, so a value beyond
    # int64 max (9223372036854775807) raises ActiveRecord::RangeError
    # (PG::NumericValueOutOfRange: value ... is out of range for type
    # bigint), an uncontracted 500 instead of the app's normal error
    # contract.
    get "/api/v1/documents", headers: bearer_header("deptA"),
        params: { offset: "9223372036854775808" }
    assert_response :success
  end

  # --- tag limits on create ---

  test "more than 50 tag entries is rejected with 422" do
    tags = 51.times.to_h { |i| [ "key#{i}", "value" ] }
    post "/api/v1/documents", headers: bearer_header("deptA"),
         params: { department_id: @department.id, file: upload_file, tags: tags }
    assert_response :unprocessable_entity
  end

  test "a tag key longer than 100 characters is rejected with 422" do
    tags = { "k" * 200 => "value" }
    post "/api/v1/documents", headers: bearer_header("deptA"),
         params: { department_id: @department.id, file: upload_file, tags: tags }
    assert_response :unprocessable_entity
  end

  test "a tag value longer than 500 characters is rejected with 422" do
    tags = { "type" => "v" * 600 }
    post "/api/v1/documents", headers: bearer_header("deptA"),
         params: { department_id: @department.id, file: upload_file, tags: tags }
    assert_response :unprocessable_entity
  end

  # --- confirmed gaps ---

  test "malformed JSON body returns an app-level JSON 400 error" do
    # In production (consider_all_requests_local = false) a ParseError raised in
    # the params middleware is re-raised by DebugExceptions and handed to
    # config.exceptions_app (JsonApiExceptions), which renders the JSON contract.
    # The test env's detailed-exceptions debug page would otherwise intercept it,
    # so we emulate production error handling for this one request only (NOT via
    # show_exceptions=:all, which would also route non-rescuable errors).
    env_config = Rails.application.env_config
    original = env_config["action_dispatch.show_detailed_exceptions"]
    env_config["action_dispatch.show_detailed_exceptions"] = false

    post "/api/v1/documents", params: '{"tags":',
         headers: { "Content-Type" => "application/json" }.merge(bearer_header("deptA"))

    assert_response :bad_request
    body = JSON.parse(response.body)
    assert body.key?("error")
  ensure
    env_config["action_dispatch.show_detailed_exceptions"] = original
  end

  test "a hostile filename is sanitized before being persisted" do
    # Desired behavior: a filename containing path separators or "../"
    # segments must not survive verbatim into the persisted Document record
    # or its Active Storage blob (both eventually flow into a
    # Content-Disposition header on download). Currently filename is only
    # presence-validated, so this is stored unsanitized.
    #
    # Scope of this test, as observed by running it without the skip:
    #   - "../" and "/" DO survive into the record — that is the gap.
    #   - A NUL byte never reaches persistence: Ruby's File layer raises
    #     ArgumentError ("path name contains null byte") inside
    #     DocumentCreator first, so it is a different failure mode and is
    #     deliberately not asserted here.
    #   - The space is deliberate and NOT asserted against: spaces are
    #     legitimate in a filename ("mon rapport.pdf"), so a fix must not
    #     strip them.
    hostile = "a b/../../etc/passwd.pdf"
    document = DocumentCreator.call(
      department_id: @department.id,
      io: StringIO.new("PDF!"),
      filename: hostile,
      content_type: "application/pdf",
      byte_size: 4,
      uploaded_by: "deptA"
    )
    assert_not_includes document.filename, "../"
    assert_not_includes document.filename, "/"
    assert_not_includes document.file.filename.to_s, "../"
    assert_not_includes document.file.filename.to_s, "/"
  end

  test "an oversized upload is rejected" do
    skip "documented finding GED-SEC-02 — see docs/security/pentest-2026-07-15.md"

    # Desired behavior: there should be a maximum accepted upload size, so a
    # Document whose byte_size is far beyond any reasonable document is
    # invalid (422 at the API layer). Currently byte_size has no validation
    # at all, so this document saves successfully instead of being rejected.
    document = Document.new(
      department: @department, filename: "huge.pdf", content_type: "application/pdf",
      byte_size: 200.megabytes, uploaded_by: "deptA"
    )
    document.file.attach(io: StringIO.new("PDF!"), filename: "huge.pdf",
                         content_type: "application/pdf")
    assert document.invalid?
    assert document.errors[:byte_size].present?
  end

  test "a legitimate filename with a space survives sanitization unchanged" do
    document = DocumentCreator.call(
      department_id: @department.id,
      io: StringIO.new("PDF!"),
      filename: "mon rapport.pdf",
      content_type: "application/pdf",
      byte_size: 4,
      uploaded_by: "deptA"
    )
    assert_equal "mon rapport.pdf", document.filename
    assert_equal "mon rapport.pdf", document.file.filename.to_s
  end

  test "a backslash path filename is reduced to its basename" do
    document = DocumentCreator.call(
      department_id: @department.id,
      io: StringIO.new("PDF!"),
      filename: "..\\..\\windows\\system32\\evil.exe",
      content_type: "application/octet-stream",
      byte_size: 4,
      uploaded_by: "deptA"
    )
    assert_equal "evil.exe", document.filename
    assert_equal "evil.exe", document.file.filename.to_s
  end

  test "a filename that sanitizes to empty falls back to unnamed" do
    document = DocumentCreator.call(
      department_id: @department.id,
      io: StringIO.new("PDF!"),
      filename: "../../",
      content_type: "application/pdf",
      byte_size: 4,
      uploaded_by: "deptA"
    )
    assert_equal "unnamed", document.filename
    assert_equal "unnamed", document.file.filename.to_s
  end
end
