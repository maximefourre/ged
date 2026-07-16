ENV["RAILS_ENV"] ||= "test"
require_relative "../config/environment"
require "rails/test_help"

require "webmock/minitest"

ENV["GED_OIDC_ISSUER"] ||= "https://idp.test"
ENV["GED_OIDC_JWKS_URL"] ||= "https://idp.test/.well-known/jwks.json"
ENV["GED_OIDC_AUDIENCE"] ||= "https://ged.test"

Dir[Rails.root.join("test/support/**/*.rb")].sort.each { |f| require f }

module ActiveSupport
  class TestCase
    # Run tests in parallel with specified workers
    parallelize(workers: :number_of_processors)

    # Setup all fixtures in test/fixtures/*.yml for all tests in alphabetical order.
    fixtures :all

    include JwtTestHelpers
    include DnsTestHelpers

    setup { Auth::JwksCache.reset! }
  end
end
