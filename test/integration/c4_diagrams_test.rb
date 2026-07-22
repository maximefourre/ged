require "test_helper"

# Guards docs/architecture/c4.md against drift, the same way api_docs_test.rb
# guards docs/api/openapi.yaml. Diagrams that silently go stale are worse than
# no diagrams, so anything the model enumerates one-by-one is enforced here:
# adding a compose service, a job or a service object fails this test until the
# diagrams are updated.
#
# Deliberately NOT enforced: things the diagrams group on purpose (the nine
# Admin controllers, the models). An over-strict test gets deleted rather than
# satisfied.
class C4DiagramsTest < ActiveSupport::TestCase
  DIAGRAM = Rails.root.join("docs/architecture/c4.md")

  setup { @doc = DIAGRAM.read }

  test "the diagram file exists and every mermaid block is closed" do
    assert DIAGRAM.exist?, "docs/architecture/c4.md is missing"
    assert_equal @doc.scan(/^```mermaid$/).size, @doc.scan(/^```$/).size,
                 "unbalanced mermaid fences in c4.md"
    assert_operator @doc.scan(/^```mermaid$/).size, :>=, 7,
                    "expected at least 7 diagrams (L1, L2, L3a, L3b, 3 dynamic views)"
  end

  test "every docker compose service appears in the container diagram" do
    services = YAML.safe_load_file(Rails.root.join("docker-compose.yml"), aliases: true)
                   .fetch("services").keys

    missing = services.reject { |name| @doc.include?(name) }
    assert_empty missing,
                 "compose services absent from c4.md: #{missing.join(", ")} — update the L2 diagram"
  end

  test "every ActiveJob appears in the worker component diagram" do
    jobs = Rails.root.glob("app/jobs/*_job.rb")
                .map { |p| p.basename(".rb").to_s.camelize }
                .reject { |name| name == "ApplicationJob" }

    missing = jobs.reject { |name| @doc.include?(name) }
    assert_empty missing,
                 "jobs absent from c4.md: #{missing.join(", ")} — update the L3b diagram"
  end

  test "every service object appears in a component diagram" do
    services = Rails.root.glob("app/services/**/*.rb").map do |path|
      path.relative_path_from(Rails.root.join("app/services")).to_s.delete_suffix(".rb").camelize
    end

    missing = services.reject { |name| @doc.include?(name) }
    assert_empty missing,
                 "service objects absent from c4.md: #{missing.join(", ")} — update the L3a or L3b diagram"
  end

  test "every recurring task class is drawn as scheduled" do
    schedule = YAML.safe_load_file(Rails.root.join("config/recurring.yml"), aliases: true) || {}
    classes = schedule.fetch("production", {}).values.filter_map { |task| task["class"] }

    missing = classes.reject { |name| @doc.include?(name) }
    assert_empty missing,
                 "recurring jobs absent from c4.md: #{missing.join(", ")} — update the L3b diagram"
  end

  test "every solid queue pool is documented" do
    queues = YAML.safe_load_file(Rails.root.join("config/queue.yml"), aliases: true)
                 .fetch("production").fetch("workers").map { |w| w.fetch("queues") }

    missing = queues.reject { |queue| @doc.match?(/Pool: #{Regexp.escape(queue)}\b/) }
    assert_empty missing,
                 "queue pools absent from c4.md: #{missing.join(", ")} — update the L3b diagram"
  end
end
