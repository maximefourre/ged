require "test_helper"

class ReconcileUploadRequestsJobTest < ActiveJob::TestCase
  setup do
    @finance = Department.create!(name: "Finance")
  end

  def create_request(status: "pending", age: 0.seconds)
    request = UploadRequest.create!(department: @finance, filename: "report.pdf",
                                    callback_url: "https://caller.test/cb",
                                    uploaded_by: "erp-app", status: status)
    request.update_columns(created_at: age.ago) if age.positive?
    request
  end

  test "re-enqueues StoreDocumentJob for stuck pending requests with a staging file" do
    request = create_request(age: 20.minutes)
    StagingStore.write(request.id, StringIO.new("PDF!"))
    ReconcileUploadRequestsJob.perform_now
    assert_enqueued_with(job: StoreDocumentJob, args: [ request.id ])
    assert_equal "pending", request.reload.status
  ensure
    StagingStore.delete(request.id)
  end

  test "fails stuck pending requests whose staging file is gone and enqueues the callback" do
    request = create_request(age: 20.minutes)
    ReconcileUploadRequestsJob.perform_now
    request.reload
    assert_equal "failed", request.status
    assert request.error_message.present?
    assert_enqueued_with(job: DeliverCallbackJob, args: [ request.id ])
  end

  test "leaves fresh pending requests alone" do
    create_request
    assert_no_enqueued_jobs { ReconcileUploadRequestsJob.perform_now }
  end

  test "purges terminal requests older than the retention window" do
    old_failed = create_request(status: "failed", age: 31.days)
    recent_completed = create_request(status: "completed", age: 1.day)
    ReconcileUploadRequestsJob.perform_now
    assert_not UploadRequest.exists?(old_failed.id)
    assert UploadRequest.exists?(recent_completed.id)
  end

  test "purges staging files older than the retention window" do
    old_path = StagingStore.write("orphan-old", StringIO.new("x"))
    File.utime(Time.now, 31.days.ago.to_time, old_path)
    fresh_path = StagingStore.write("orphan-fresh", StringIO.new("x"))
    ReconcileUploadRequestsJob.perform_now
    assert_not File.exist?(old_path)
    assert File.exist?(fresh_path)
  ensure
    StagingStore.delete("orphan-old")
    StagingStore.delete("orphan-fresh")
  end
end
