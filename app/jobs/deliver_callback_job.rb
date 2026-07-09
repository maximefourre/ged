require "net/http"

class DeliverCallbackJob < ApplicationJob
  class DeliveryFailed < StandardError; end

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
      document_id: upload_request.document_id
    }
    response = Net::HTTP.post(URI(upload_request.callback_url), payload.to_json,
                              "Content-Type" => "application/json")
    unless response.is_a?(Net::HTTPSuccess)
      raise DeliveryFailed, "callback returned HTTP #{response.code}"
    end
  rescue Errno::ECONNREFUSED, Errno::ETIMEDOUT, SocketError, Timeout::Error, EOFError => e
    raise DeliveryFailed, "#{e.class}: #{e.message}"
  end
end
