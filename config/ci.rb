# Run using bin/ci

require "net/http"
require "open3"
require "uri"

# Parity talks to this worktree's compose stack. It runs only when that stack
# answers GET /up. GED_BACKEND is the stack `bin/worktree-stack` would use
# (`active`: GED_STACK, else the name `up` remembered, else rust). A stack
# that does not answer is an explicit skip, not a passing step.
module CiParity
  module_function

  def docker_daemon?
    system("docker info > /dev/null 2>&1")
  end

  def up_url
    stdout, _stderr, status = Open3.capture3("bin/worktree-stack", "urls")
    return nil unless status.success?

    port = stdout[/web=(\d+)/, 1]
    return nil unless port

    "http://127.0.0.1:#{port}/up"
  end

  def responds?(url)
    uri = URI(url)
    Net::HTTP.start(uri.host, uri.port, open_timeout: 3, read_timeout: 3) do |http|
      http.get(uri.request_uri).is_a?(Net::HTTPSuccess)
    end
  rescue Errno::ECONNREFUSED, Errno::EHOSTUNREACH, Errno::EADDRNOTAVAIL,
         Errno::ETIMEDOUT, SocketError, IOError, Timeout::Error, Net::ProtocolError
    false
  end

  # [name, detail]. name is "rust" or "rails"; nil when the helper failed.
  def active_backend
    stdout, stderr, status = Open3.capture3("bin/worktree-stack", "active")
    name = stdout.strip
    return [ name, nil ] if status.success? && %w[rust rails].include?(name)

    detail = [ stdout, stderr ].join.strip
    detail = "bin/worktree-stack active failed (#{status.exitstatus})" if detail.empty?
    [ nil, detail ]
  end
end

CI.run do
  step "Setup", "bin/setup --skip-server"

  step "Style: Ruby", "bin/rubocop"

  step "Security: Gem audit", "bin/bundler-audit"
  step "Security: Importmap vulnerability audit", "bin/importmap audit"
  step "Security: Brakeman code analysis", "bin/brakeman --quiet --no-pager --exit-on-warn --exit-on-error"
  step "Tests: Rails", "bin/rails test"
  step "Tests: System", "bin/rails test:system"
  step "Rust: fmt", "cd rust && mise exec -- cargo fmt --check"
  step "Rust: clippy", "cd rust && mise exec -- cargo clippy --all-targets -- -D warnings"
  step "Rust: tests", "cd rust && mise exec -- cargo test -p ged"

  # Image builds and the stack probe need a daemon. The GitHub rust job
  # builds the same two Dockerfiles and does not run parity (no compose
  # stack there). Skip both here when Docker does not answer.
  docker_up = CiParity.docker_daemon?
  url = CiParity.up_url if docker_up
  if docker_up && url && CiParity.responds?(url)
    backend, detail = CiParity.active_backend
    if backend
      step "Parity: #{backend}", "cd rust && env GED_BACKEND=#{backend} mise exec -- cargo test -p parity"
    else
      heading "Parity: backend", detail, type: :error
      results << false
    end
  elsif docker_up
    probed = url || "(no web port from bin/worktree-stack urls)"
    heading "Parity: skipped",
      "GET #{probed} did not succeed, so cargo test -p parity was not run. " \
      "Start the stack with bin/worktree-stack up && bin/worktree-stack seed",
      type: :error
  else
    heading "Parity: skipped",
      "Docker did not answer, so the worktree stack was not probed and cargo test -p parity was not run",
      type: :error
  end

  step "Docker: compose config", "bin/worktree-stack config --quiet"
  step "Docker: compose config (rails)", "env GED_STACK=rails bin/worktree-stack config --quiet"
  if docker_up
    step "Docker: build Rust image", "docker build -t ged:ci ."
    step "Docker: build Rails image", "docker build -f Dockerfile.rails -t ged:ci-rails ."
  end
  step "Tests: Seeds", "env RAILS_ENV=test bin/rails db:seed:replant"

  # Optional: set a green GitHub commit status to unblock PR merge.
  # Requires the `gh` CLI and `gh extension install basecamp/gh-signoff`.
  # if success?
  #   step "Signoff: All systems go. Ready for merge and deploy.", "gh signoff"
  # else
  #   failure "Signoff: CI failed. Do not merge or deploy.", "Fix the issues and try again."
  # end
end
