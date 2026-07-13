require "test_helper"

# config/queue.yml lists explicit queues instead of "*" (a "*" pool is
# claimed as one global FIFO, letting an uploads backlog starve default).
# The flip side: a job whose queue no worker pool drains would sit enqueued
# forever — this guard fails when a `queue_as` isn't covered.
class QueueCoverageTest < ActiveSupport::TestCase
  test "every job queue is drained by a worker pool in every environment" do
    yaml = ERB.new(Rails.root.join("config/queue.yml").read).result
    config = YAML.safe_load(yaml, aliases: true)

    Rails.application.eager_load!
    job_classes = ApplicationJob.descendants +
                  [ ActiveStorage::PurgeJob, ActiveStorage::AnalyzeJob ]
    job_queues = job_classes.map { |job| job.new.queue_name.to_s }.uniq

    config.each do |env, env_config|
      workers = env_config.is_a?(Hash) ? env_config["workers"] : nil
      next if workers.blank?

      covered = workers.flat_map { |w| Array(w["queues"]).map(&:to_s) }
      assert_not_includes covered, "*",
        "queue.yml (#{env}) reintroduces a '*' pool — see the starvation comment there"

      uncovered = job_queues - covered
      assert_empty uncovered,
        "Queues #{uncovered.inspect} have no worker pool in config/queue.yml (#{env}) — " \
        "jobs enqueued there would never run"
    end
  end
end
