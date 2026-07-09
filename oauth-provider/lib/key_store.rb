require "openssl"
require "jwt"
require "fileutils"

module OauthProvider
  # Generates an RSA signing key on first boot and persists it so tokens
  # survive restarts. kid is derived from the JWK thumbprint.
  module KeyStore
    module_function

    def data_dir
      dir = ENV.fetch("OAUTH_DATA_DIR", File.expand_path("../data", __dir__))
      FileUtils.mkdir_p(dir)
      dir
    end

    def signing_key
      @signing_key ||= begin
        path = File.join(data_dir, "signing_key.pem")
        if File.exist?(path)
          OpenSSL::PKey::RSA.new(File.read(path))
        else
          key = OpenSSL::PKey::RSA.new(2048)
          File.write(path, key.to_pem)
          File.chmod(0o600, path)
          key
        end
      end
    end

    def jwk
      @jwk ||= JWT::JWK.new(signing_key, { use: "sig", alg: "RS256" })
    end

    def jwks
      { keys: [jwk.export] } # export without private key
    end
  end
end
