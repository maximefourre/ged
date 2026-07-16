require "net/http"

class DeliverCallbackJob < ApplicationJob
  class DeliveryFailed < StandardError; end

  OPEN_TIMEOUT = 5
  READ_TIMEOUT = 10

  queue_as :default

  retry_on DeliveryFailed, wait: :polynomially_longer, attempts: 5 do |job, error|
    Telemetry.callback_deliveries.add(1, attributes: { "outcome" => "exhausted" })
    Rails.logger.error(
      "Callback delivery permanently failed for upload_request #{job.arguments.first}: #{error.message}"
    )
  end

  def perform(upload_request_id)
    upload_request = UploadRequest.find(upload_request_id)
    uri = URI(upload_request.callback_url)
    # SSRF barrier: resolve + vet once, then PIN the approved address so the
    # socket connects to exactly what we checked (no re-resolution — closes the
    # rebinding TOCTOU). A blocked or unresolvable target is refused outright
    # (no DeliveryFailed — a forbidden destination must not be retried).
    decision = CallbackUrlPolicy.decide(upload_request.callback_url, resolve: true)
    if (reason = decision.reason)
      Telemetry.callback_deliveries.add(1, attributes: { "outcome" => "refused" })
      Rails.logger.error(
        "Callback refused for upload_request #{upload_request.id}: callback_url #{reason}"
      )
      return
    end

    payload = {
      request_id: upload_request.id,
      status: upload_request.status,
      document_id: upload_request.document_id,
      error_message: upload_request.error_message,
      tags: upload_request.document&.tags,
      metadata: upload_request.document&.metadata_hash
    }

    http = Net::HTTP.new(uri.hostname, uri.port)
    http.ipaddr = decision.ipaddr # pin the vetted address; keeps hostname for Host/SNI/cert
    http.use_ssl = uri.scheme == "https"
    http.open_timeout = OPEN_TIMEOUT
    http.read_timeout = READ_TIMEOUT
    response = http.start do |conn|
      conn.post(uri.request_uri, payload.to_json, "Content-Type" => "application/json")
    end
    unless response.is_a?(Net::HTTPSuccess)
      fail_delivery("callback returned HTTP #{response.code}")
    end
    Telemetry.callback_deliveries.add(1, attributes: { "outcome" => "success" })
  rescue SystemCallError, SocketError, IOError, OpenSSL::SSL::SSLError, Timeout::Error => e
    fail_delivery("#{e.class}: #{e.message}")
  end

  private

  # Failures are counted per attempt; the final attempt additionally emits
  # "exhausted" via the retry_on block above.
  def fail_delivery(message)
    Telemetry.callback_deliveries.add(1, attributes: { "outcome" => "failure" })
    raise DeliveryFailed, message
  end
end
