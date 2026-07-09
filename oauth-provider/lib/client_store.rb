require "sqlite3"
require "securerandom"
require "digest"

module OauthProvider
  # SQLite-backed client registry. Secrets are stored as SHA-256 digests —
  # fine for a dev-only stopgap, NOT a production credential store.
  module ClientStore
    module_function

    def db
      @db ||= begin
        db = SQLite3::Database.new(File.join(KeyStore.data_dir, "clients.sqlite3"))
        db.execute <<~SQL
          CREATE TABLE IF NOT EXISTS clients (
            client_id TEXT PRIMARY KEY,
            secret_digest TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
          )
        SQL
        db
      end
    end

    def register(client_id)
      secret = SecureRandom.hex(32)
      db.execute("INSERT OR REPLACE INTO clients (client_id, secret_digest) VALUES (?, ?)",
                 [ client_id, Digest::SHA256.hexdigest(secret) ])
      secret
    end

    def authenticate(client_id, client_secret)
      return false if client_id.to_s.empty? || client_secret.to_s.empty?
      row = db.get_first_row("SELECT secret_digest FROM clients WHERE client_id = ?", [ client_id ])
      return false unless row
      expected = row[0]
      actual = Digest::SHA256.hexdigest(client_secret)
      OpenSSL.secure_compare(expected, actual)
    end

    def reset_for_tests!
      db.execute("DELETE FROM clients")
    end
  end
end
