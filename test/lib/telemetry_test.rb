require "test_helper"

class TelemetryTest < ActiveSupport::TestCase
  # The OTel initializer must stay dormant in test (no OTEL_EXPORTER_OTLP_ENDPOINT):
  # if the SDK ever gets loaded here, exports would trip WebMock and every
  # instrument call would grow a real cost. Regression guard.
  test "the OpenTelemetry SDK is not loaded when no OTLP endpoint is configured" do
    assert_not defined?(OpenTelemetry::SDK)
  end

  test "instruments are no-ops that accept adds and records without raising" do
    assert_nothing_raised do
      Telemetry.documents_uploaded.add(1, attributes: { "upload_mode" => "sync" })
      Telemetry.callback_deliveries.add(1, attributes: { "outcome" => "success" })
      Telemetry.job_duration.record(0.5, attributes: { "job.class" => "StoreDocumentJob" })
    end
  end

  test "instruments are memoized" do
    assert_same Telemetry.documents_uploaded, Telemetry.documents_uploaded
  end
end
