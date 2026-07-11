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

  test "applies the requested tags to the created document" do
    request = UploadRequest.create!(department: @finance, filename: "report.pdf",
                                    callback_url: "https://caller.test/cb",
                                    uploaded_by: "erp-app",
                                    tags: { "type" => "facture" })
    StagingStore.write(request.id, StringIO.new("PDF!"))

    StoreDocumentJob.perform_now(request.id)

    assert_equal({ "type" => "facture" }, Document.find(request.reload.document_id).tags)
  end

  test "concurrent double execution stores the document only once" do
    # simulate a reconcile-re-enqueued copy starting while the first run is
    # mid-storage (queue backlog > STUCK_AFTER): it must back off, not store
    # a second document or send a second callback
    request_id = @request.id
    original = DocumentCreator.method(:call)
    interleaved = false
    DocumentCreator.define_singleton_method(:call) do |**kwargs|
      unless interleaved
        interleaved = true
        StoreDocumentJob.perform_now(request_id)
      end
      original.call(**kwargs)
    end
    begin
      assert_enqueued_jobs(1, only: DeliverCallbackJob) do
        StoreDocumentJob.perform_now(request_id)
      end
    ensure
      DocumentCreator.define_singleton_method(:call, original)
    end
    assert_equal 1, Document.count
    assert_equal "completed", @request.reload.status
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

  test "runs on the dedicated uploads queue" do
    assert_equal "uploads", StoreDocumentJob.new.queue_name
  end
end
