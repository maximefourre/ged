# Captures emissions to a Telemetry instrument without a real OTel SDK
# (minitest/mock is no longer bundled with minitest 6, hence the hand-rolled
# singleton swap — same pattern as DnsTestHelpers; forked workers make the
# global swap safe).
module TelemetryTestHelpers
  # calls = capture_telemetry(:documents_uploaded) { ... }
  # #=> [[1, { "upload_mode" => "sync" }], ...] for every .add/.record
  def capture_telemetry(instrument_name)
    calls = []
    recorder = Object.new
    recorder.define_singleton_method(:add) { |value, attributes: {}| calls << [ value, attributes ] }
    recorder.define_singleton_method(:record) { |value, attributes: {}| calls << [ value, attributes ] }

    original = Telemetry.method(instrument_name)
    Telemetry.define_singleton_method(instrument_name) { recorder }
    yield
    calls
  ensure
    Telemetry.singleton_class.remove_method(instrument_name)
    Telemetry.define_singleton_method(instrument_name, original)
  end
end
