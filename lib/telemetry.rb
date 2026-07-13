# Central home for GED's custom OpenTelemetry instruments. Only the metrics
# API gem is loaded at boot, so without a configured SDK (see
# config/initializers/opentelemetry.rb) these are no-op instruments and every
# .add/.record is a few method dispatches — safe to call unconditionally.
#
# Cardinality discipline: attribute values must come from closed sets
# (upload_mode, outcome, job class). Never filenames, tags, departments, or
# URLs — they explode metric cardinality and can carry PII.
module Telemetry
  module_function

  def meter
    @meter ||= OpenTelemetry.meter_provider.meter("ged")
  end

  def documents_uploaded
    @documents_uploaded ||= meter.create_counter(
      "ged.documents.uploaded", unit: "{document}",
      description: "Documents created, by storage mode (attribute upload_mode)"
    )
  end

  def callback_deliveries
    @callback_deliveries ||= meter.create_counter(
      "ged.callbacks.deliveries", unit: "{attempt}",
      description: "Callback delivery attempts by outcome " \
                   "(success/failure/refused/exhausted; failure counts per attempt)"
    )
  end

  def job_duration
    @job_duration ||= meter.create_histogram(
      "ged.job.duration", unit: "s",
      description: "Active Job perform duration"
    )
  end
end
