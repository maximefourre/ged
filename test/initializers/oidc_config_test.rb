require "test_helper"

class OidcConfigTest < ActiveSupport::TestCase
  # The initializer refactors the boot guard into OidcConfig.verify! so it can
  # be exercised here without rebooting the app. GED_OIDC_AUDIENCE is now a
  # required var: production must refuse to boot without it (GED-SEC-01).
  def production_env
    ActiveSupport::StringInquirer.new("production")
  end

  test "verify! raises in production when GED_OIDC_AUDIENCE is absent" do
    with_env("GED_OIDC_AUDIENCE" => nil) do
      error = assert_raises(RuntimeError) { OidcConfig.verify!(env: production_env) }
      assert_match(/GED_OIDC_AUDIENCE/, error.message)
    end
  end

  test "verify! passes in production when all required vars are present" do
    with_env("GED_OIDC_AUDIENCE" => "https://ged.test") do
      assert_nothing_raised { OidcConfig.verify!(env: production_env) }
    end
  end

  test "verify! never raises in the test environment even if a var is missing" do
    with_env("GED_OIDC_AUDIENCE" => nil) do
      assert_nothing_raised do
        OidcConfig.verify!(env: ActiveSupport::StringInquirer.new("test"))
      end
    end
  end

  private

  # Save/restore ENV around the block so a nil override deletes-and-restores.
  def with_env(overrides)
    original = overrides.keys.index_with { |k| ENV[k] }
    overrides.each { |k, v| v.nil? ? ENV.delete(k) : ENV[k] = v }
    yield
  ensure
    original.each { |k, v| v.nil? ? ENV.delete(k) : ENV[k] = v }
  end
end
