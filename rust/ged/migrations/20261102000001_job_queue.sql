-- T8: Postgres-backed job queue, in the same database as the business data so
-- that enqueueing shares the caller's transaction (replaces solid_queue_*).

CREATE TABLE jobs (
  id bigserial PRIMARY KEY,
  queue text NOT NULL CHECK (queue IN ('uploads', 'default')),
  kind text NOT NULL,
  args jsonb NOT NULL DEFAULT '{}'::jsonb,
  attempts integer NOT NULL DEFAULT 0,
  run_at timestamptz NOT NULL DEFAULT now(),
  claimed_by uuid,
  claimed_at timestamptz,
  finished_at timestamptz,
  failed_at timestamptz,
  last_error text,
  created_at timestamptz NOT NULL DEFAULT now()
);

-- Claimable jobs, in FIFO order per queue.
CREATE INDEX index_jobs_ready ON jobs (queue, id)
  WHERE finished_at IS NULL AND failed_at IS NULL AND claimed_by IS NULL;
CREATE INDEX index_jobs_claimed_by ON jobs (claimed_by) WHERE claimed_by IS NOT NULL;
CREATE INDEX index_jobs_finished_at ON jobs (finished_at) WHERE finished_at IS NOT NULL;

-- A row = the queue is paused, for every replica.
CREATE TABLE queue_pauses (
  queue text PRIMARY KEY CHECK (queue IN ('uploads', 'default')),
  paused_at timestamptz NOT NULL DEFAULT now()
);

-- One row per worker process; stale heartbeat = dead worker.
CREATE TABLE job_workers (
  id uuid PRIMARY KEY,
  pool text NOT NULL,
  hostname text NOT NULL,
  pid integer NOT NULL,
  started_at timestamptz NOT NULL DEFAULT now(),
  last_heartbeat_at timestamptz NOT NULL DEFAULT now()
);

-- Scheduler de-duplication: the first replica to insert a slot enqueues it.
CREATE TABLE scheduled_runs (
  task_key text NOT NULL,
  run_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (task_key, run_at)
);
