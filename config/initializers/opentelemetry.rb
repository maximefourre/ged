# OpenTelemetry is only initialised when an OTLP endpoint is configured
# (standard env var), so dev/test stay hermetic (WebMock) and a deployment
# without a backend pays nothing: no gems loaded, no monkey-patching, no
# exporter threads. lib/telemetry.rb uses the always-loaded metrics API,
# whose instruments are no-ops until the SDK below takes over.
#
# SOLID_QUEUE_IN_PUMA runs jobs in this same process, so this single init
# covers web requests and jobs alike. Puma cluster mode (WEB_CONCURRENCY) is
# handled by the SDK's fork hooks (opentelemetry-metrics-sdk ForkHooks;
# BatchSpanProcessor is fork-aware).
#
# If in-process Solid Queue polling (0.1s/1s SELECTs) floods the backend with
# tiny root spans, the escape hatches are OTEL_TRACES_SAMPLER=
# parentbased_traceidratio + OTEL_TRACES_SAMPLER_ARG, or dropping the PG
# instrumentation (ActiveRecord spans remain).
if ENV["OTEL_EXPORTER_OTLP_ENDPOINT"].present? && ENV["OTEL_SDK_DISABLED"] != "true"
  require "opentelemetry-sdk"
  require "opentelemetry-metrics-sdk"            # patches SDK.configure below
  require "opentelemetry-exporter-otlp"
  require "opentelemetry-exporter-otlp-metrics"  # picked up via OTEL_METRICS_EXPORTER (default "otlp")
  require "opentelemetry-instrumentation-rails"
  require "opentelemetry-instrumentation-pg"
  require "opentelemetry-instrumentation-net_http"

  OpenTelemetry::SDK.configure do |c|
    c.service_name = ENV.fetch("OTEL_SERVICE_NAME", "ged")
    c.use_all(
      # Never ship literal SQL: tag and filename values appear in WHEREs.
      "OpenTelemetry::Instrumentation::PG" => { db_statement: :obfuscate }
    )
  end

  # Cheap aggregable per-job metric next to the active_job spans. Bounded
  # cardinality: a handful of job classes x two queues.
  ActiveSupport::Notifications.subscribe("perform.active_job") do |event|
    job = event.payload[:job]
    Telemetry.job_duration.record(
      event.duration / 1000.0,
      attributes: { "job.class" => job.class.name, "queue" => job.queue_name }
    )
  end

  # The SDK installs no exit hooks itself; flush buffered spans/metrics.
  at_exit do
    OpenTelemetry.tracer_provider.shutdown(timeout: 3)
    OpenTelemetry.meter_provider.shutdown(timeout: 3)
  end
end
