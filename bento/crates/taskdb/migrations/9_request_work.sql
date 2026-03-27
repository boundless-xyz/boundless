--------------------------------------------------------------------------------
-- 1. Add job_created_at column to tasks
--------------------------------------------------------------------------------

ALTER TABLE tasks ADD COLUMN IF NOT EXISTS job_created_at TIMESTAMP;

-- Backfill: set job_created_at from each job's init task created_at
UPDATE tasks t
SET job_created_at = init.created_at
FROM tasks init
WHERE init.job_id = t.job_id
  AND init.task_id = 'init'
  AND t.job_created_at IS NULL;

-- Fallback for any tasks without an init task (shouldn't happen, but be safe)
UPDATE tasks
SET job_created_at = created_at
WHERE job_created_at IS NULL;

ALTER TABLE tasks ALTER COLUMN job_created_at SET NOT NULL;
ALTER TABLE tasks ALTER COLUMN job_created_at SET DEFAULT NOW();

--------------------------------------------------------------------------------
-- 2. Replace partial index to include job_created_at for index-only ordering
--    Previous index (from migration 2): tasks_ready_idx ON tasks(stream_id, created_at) WHERE state = 'ready'
--    New index: tasks_ready_idx ON tasks(stream_id, job_created_at, created_at) WHERE state = 'ready'
--------------------------------------------------------------------------------

DROP INDEX IF EXISTS tasks_ready_idx;
CREATE INDEX tasks_ready_idx ON tasks(stream_id, job_created_at, created_at) WHERE state = 'ready';

--------------------------------------------------------------------------------
-- 3. Update create_job to set job_created_at on the init task
--    Change from migration 1: adds created_at and job_created_at to the INSERT
--------------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION create_job(
    stream_id UUID,
    task_def jsonb,
    max_retries INTEGER,
    timeout_secs INTEGER,
    user_id TEXT)
  RETURNS UUID as $$
DECLARE
  new_id UUID;
  job_ts TIMESTAMP; -- CHANGED: new variable for job creation timestamp
BEGIN
  INSERT INTO jobs (state, user_id) VALUES ('running', user_id) RETURNING (id) INTO new_id;
  job_ts := NOW();
  -- CHANGED: added created_at and job_created_at columns (both set to job_ts)
  INSERT INTO tasks (job_id, task_id, stream_id, task_def, prerequisites, max_retries, timeout_secs, state, waiting_on, created_at, job_created_at)
         VALUES (new_id, 'init', stream_id, task_def, '[]', max_retries, timeout_secs, 'ready', 0, job_ts, job_ts);
  RETURN new_id;
END;
$$ LANGUAGE plpgsql;

--------------------------------------------------------------------------------
-- 4. Update create_task to look up and set job_created_at from the init task
--    Change from migration 1: looks up job_created_at and includes it in INSERT
--------------------------------------------------------------------------------

CREATE OR REPLACE PROCEDURE create_task(
    job_id_var UUID,
    task_id_var TEXT,
    stream_id UUID,
    task_def jsonb,
    prerequisites jsonb,
    max_retries INTEGER,
    timeout_secs INTEGER) as $$
DECLARE
    not_done_count INTEGER;
    job_ts TIMESTAMP; -- CHANGED: new variable for job creation timestamp
BEGIN
  -- CHANGED: look up the job's creation timestamp from the init task
  SELECT t.job_created_at INTO job_ts
  FROM tasks t
  WHERE t.job_id = job_id_var AND t.task_id = 'init';

  IF job_ts IS NULL THEN
    job_ts := NOW();
  END IF;

  -- CHANGED: added job_created_at column
  INSERT INTO tasks (job_id, task_id, stream_id, task_def, prerequisites, max_retries, timeout_secs, state, waiting_on, job_created_at)
         VALUES (job_id_var, task_id_var, stream_id, task_def, prerequisites, max_retries, timeout_secs, 'pending', 0, job_ts);
  INSERT INTO task_deps
  SELECT job_id_var, value#>>'{}', task_id_var FROM jsonb_array_elements(prerequisites);

  SELECT COUNT(*) INTO not_done_count FROM task_deps, tasks
         WHERE task_deps.job_id = job_id_var and
               task_deps.post_task_id = task_id_var and
               tasks.job_id = job_id_var AND
               tasks.task_id = task_deps.pre_task_id and
               tasks.state != 'done';

  UPDATE tasks SET
      waiting_on = not_done_count,
      state = (CASE not_done_count WHEN 0 THEN 'ready' ELSE 'pending' END)::task_state
  WHERE job_id = job_id_var AND task_id = task_id_var;
END;
$$ LANGUAGE plpgsql;

--------------------------------------------------------------------------------
-- 5. Update request_work
--    Changes from migration 6:
--    a) Stream selection: was `ORDER BY priority, reserved DESC`
--       now `WHERE streams.ready > 0 ORDER BY reserved DESC`
--       (skip streams with no ready tasks, prioritize by reserved capacity)
--    b) Task ordering: was correlated subquery
--         `ORDER BY (SELECT MIN(t2.created_at) FROM tasks t2 WHERE t2.job_id = tasks.job_id)`
--       now uses denormalized column
--         `ORDER BY job_created_at ASC, created_at ASC`
--       (same job-level FIFO semantics, but index-backed instead of full table scan)
--------------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION request_work(in_worker_type TEXT)
  RETURNS TABLE (job_id UUID, task_id TEXT, task_def jsonb, prereqs jsonb, max_retries INTEGER) as $$
DECLARE
  stream UUID;
  found_job_id UUID;
  found_task_id TEXT;
  found_definition jsonb;
  found_max_retries INTEGER;
  prereq_outputs jsonb;
BEGIN
  SELECT id INTO stream
  FROM streams
  WHERE streams.worker_type = in_worker_type
  AND streams.ready > 0 -- CHANGED: new filter, skip streams with no ready tasks
  ORDER BY reserved DESC -- CHANGED: was `ORDER BY priority, reserved DESC`
  LIMIT 1
  FOR UPDATE SKIP LOCKED;

  IF stream IS NOT NULL THEN
    WITH selected_task AS (
      SELECT tasks.job_id, tasks.task_id, tasks.task_def, tasks.max_retries
      FROM tasks
      WHERE tasks.stream_id = stream
        AND tasks.state = 'ready'
      -- CHANGED: was correlated subquery `(SELECT MIN(t2.created_at) FROM tasks t2 ...)`
      -- now uses denormalized job_created_at column (index-backed, no table scan)
      ORDER BY job_created_at ASC, created_at ASC
      LIMIT 1
      FOR UPDATE SKIP LOCKED
    ),
    update_task AS (
      UPDATE tasks
      SET state = 'running',
          started_at = now()
      FROM selected_task
      WHERE tasks.job_id = selected_task.job_id
        AND tasks.task_id = selected_task.task_id
      RETURNING selected_task.*
    )
    SELECT INTO found_job_id, found_task_id, found_definition, found_max_retries
      update_task.job_id, update_task.task_id, update_task.task_def, update_task.max_retries
    FROM update_task;
  END IF;

  IF found_job_id is NOT NULL THEN
    job_id := found_job_id;
    task_id := found_task_id;
    task_def := found_definition;
    max_retries := found_max_retries;
    prereqs := '[]';
    RETURN NEXT;
  END IF;
END;
$$ LANGUAGE plpgsql;
