require "test_helper"

class DocumentCreatorTest < ActiveSupport::TestCase
  include TelemetryTestHelpers

  setup do
    @finance = Department.create!(name: "Finance")
  end

  test "emits a documents_uploaded count with upload_mode=sync" do
    calls = capture_telemetry(:documents_uploaded) do
      create_document
    end
    assert_equal [ [ 1, { "upload_mode" => "sync" } ] ], calls
  end

  test "emits upload_mode=async for async-stored documents" do
    calls = capture_telemetry(:documents_uploaded) do
      create_document(stored_async: true)
    end
    assert_equal [ [ 1, { "upload_mode" => "async" } ] ], calls
  end

  test "emits nothing when the document is invalid" do
    calls = capture_telemetry(:documents_uploaded) do
      assert_raises(ActiveRecord::RecordInvalid) { create_document(department_id: nil) }
    end
    assert_empty calls
  end

  private

  def create_document(department_id: @finance.id, stored_async: false)
    DocumentCreator.call(department_id: department_id, io: StringIO.new("pdf bytes"),
                         filename: "a.pdf", content_type: "application/pdf",
                         byte_size: 9, uploaded_by: "erp-app", stored_async: stored_async)
  end
end
