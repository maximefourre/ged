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
    assert_enqueued_with(job: DeliverCallbackJob, args: [ @request.id ])
  end

  test "marks the request failed on error and still enqueues the callback" do
    StagingStore.delete(@request.id) # provoke a failure: staged file missing
    StoreDocumentJob.perform_now(@request.id)
    @request.reload
    assert_equal "failed", @request.status
    assert @request.error_message.present?
    assert_nil @request.document_id
    assert_enqueued_with(job: DeliverCallbackJob, args: [ @request.id ])
  end
end
