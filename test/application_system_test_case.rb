require "test_helper"

# Headless Chrome intermittently raises "Node with given id does not belong to
# the document" right after a navigation, before the DOM node cache settles.
# Capybara auto-retries its own "invalid element" errors within the wait window
# but not Selenium::WebDriver::Error::UnknownError, so teach the selenium driver
# to treat it as retryable. A genuinely persistent error still re-raises once the
# wait time is exhausted.
module CapybaraSeleniumStaleNodeRetry
  def invalid_element_errors
    super + [ Selenium::WebDriver::Error::UnknownError ]
  end
end
Capybara::Selenium::Driver.prepend(CapybaraSeleniumStaleNodeRetry)

class ApplicationSystemTestCase < ActionDispatch::SystemTestCase
  # Real browser system tests are heavy and share a single app server, so run
  # them serially rather than inheriting the parallel worker pool from
  # ActiveSupport::TestCase (configured in test_helper.rb).
  parallelize(workers: 1)

  driven_by :selenium, using: :headless_chrome, screen_size: [ 1400, 1400 ]

  # Give the browser a little more slack than Capybara's 2s default to keep the
  # first (cold) page loads from flaking.
  Capybara.default_max_wait_time = 5

  # test_helper.rb loads webmock/minitest, which blocks all real HTTP. A real
  # browser needs to reach chromedriver and the Capybara app server, so allow
  # live connections during system tests. On teardown, restore blocking of
  # external hosts but keep localhost open so the chromedriver shutdown at
  # process exit isn't blocked.
  setup { WebMock.allow_net_connect! }
  teardown { WebMock.disable_net_connect!(allow_localhost: true) }

  # Every system test logs in from the same IP through the same in-process app
  # server, so the login rate limit accumulates across tests — reset it.
  setup { Admins::SessionsController::RATE_LIMIT_STORE.clear }

  # Sign in through the real Devise login form so the whole auth path is
  # exercised, and wait for the post-login documents list before returning so
  # callers don't race the redirect. Assumes an Admin with these credentials
  # exists (created in setup).
  def sign_in_admin(username: "admin", password: "changeme")
    visit "/admin/documents"
    fill_in "Username", with: username
    fill_in "Password", with: password
    click_button "Log in"
    assert_selector "h1", text: "Documents"
  end
end
