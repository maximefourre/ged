require "net/http"

class DeliverCallbackJob < ApplicationJob
  class DeliveryFailed < StandardError; end

  OPEN_TIMEOUT = 5
  READ_TIMEOUT = 10

  queue_as :default

  retry_on DeliveryFailed, wait: :polynomially_longer, attempts: 5 do |job, error|
    Rails.logger.error(
      "Callback delivery permanently failed for upload_request #{job.arguments.first}: #{error.message}"
    )
  end

  def perform(upload_request_id)
    upload_request = UploadRequest.find(upload_request_id)
    payload = {
      request_id: upload_request.id,
      status: upload_request.status,
      document_id: upload_request.document_id,
      error_message: upload_request.error_message,
      tags: upload_request.document&.tags
    }
    uri = URI(upload_request.callback_url)
    response = Net::HTTP.start(uri.host, uri.port,
                               use_ssl: uri.scheme == "https",
                               open_timeout: OPEN_TIMEOUT, read_timeout: READ_TIMEOUT) do |http|
      http.post(uri.request_uri, payload.to_json, "Content-Type" => "application/json")
    end
    unless response.is_a?(Net::HTTPSuccess)
      raise DeliveryFailed, "callback returned HTTP #{response.code}"
    end
  rescue SystemCallError, SocketError, IOError, OpenSSL::SSL::SSLError, Timeout::Error => e
    raise DeliveryFailed, "#{e.class}: #{e.message}"
  end
end
