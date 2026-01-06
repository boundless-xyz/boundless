-- Migration 7: Add batch work request and failed job cleanup
--
-- This migration adds:
-- 1. request_work_batch() function for pulling multiple tasks at once
-- 2. fail_tasks_from_failed_jobs() function for periodic cleanup

-- Drop and recreate create_task
DROP PROCEDURE IF EXISTS create_task;

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
BEGIN
  INSERT INTO tasks (job_id, task_id, stream_id, task_def, prerequisites, max_retries, timeout_secs, state, waiting_on)
         VALUES (job_id_var, task_id_var, stream_id, task_def, prerequisites, max_retries, timeout_secs, 'pending', 0);
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

-- Update update_task_done
DROP FUNCTION IF EXISTS update_task_done;

CREATE OR REPLACE FUNCTION update_task_done(
  job_id_var UUID,
  task_id_var TEXT,
  output_var jsonb
)
RETURNS BOOLEAN as $$
DECLARE
  found_done_task BOOLEAN DEFAULT FALSE;
  unresolved INTEGER;
BEGIN
  UPDATE tasks SET
    state = 'done',
    output = output_var,
    updated_at = now(),
    progress = 1.0
    WHERE
      tasks.job_id = job_id_var and
      tasks.task_id = task_id_var and
      (state = 'ready' OR state = 'running');
  found_done_task = FOUND;

  -- Reduce deps if a task is done, and maybe move to ready
  IF found_done_task THEN
    -- In a single updatable CTE, lock and decrement waiting_on for child tasks
    WITH child_tasks AS (
      SELECT tasks.job_id, tasks.task_id, tasks.waiting_on
        FROM tasks
        JOIN task_deps
          ON task_deps.job_id = tasks.job_id
          AND task_deps.post_task_id = tasks.task_id
        WHERE tasks.job_id = job_id_var
          AND task_deps.job_id = job_id_var
          AND task_deps.pre_task_id = task_id_var
          AND tasks.state != 'failed'
        FOR UPDATE
    )
    UPDATE tasks SET
      waiting_on = child_tasks.waiting_on - 1,
      state = (CASE WHEN child_tasks.waiting_on - 1 <= 0
                  THEN 'ready'
                  ELSE 'pending'
              END)::task_state
    FROM child_tasks
    WHERE tasks.job_id = child_tasks.job_id AND
      tasks.task_id = child_tasks.task_id;

    SELECT COUNT(*) INTO unresolved FROM tasks WHERE job_id = job_id_var AND state != 'done';
    IF unresolved = 0 THEN
      UPDATE jobs SET state = 'done' WHERE id = job_id_var;
    END IF;
  END IF;

  RETURN found_done_task;
END;
$$ LANGUAGE plpgsql;

-- Create batch work request function
CREATE OR REPLACE FUNCTION request_work_batch(in_worker_type TEXT, batch_size INTEGER)
  RETURNS TABLE (job_id UUID, task_id TEXT, task_def jsonb, prereqs jsonb, max_retries INTEGER) as $$
DECLARE
  stream UUID;
BEGIN
  SELECT id INTO stream
  FROM streams
  WHERE streams.worker_type = in_worker_type
  ORDER BY priority, reserved DESC
  LIMIT 1
  FOR UPDATE SKIP LOCKED;

  IF stream IS NOT NULL THEN
    RETURN QUERY
    WITH selected_tasks AS (
      SELECT tasks.job_id, tasks.task_id, tasks.task_def, tasks.max_retries
      FROM tasks
      JOIN jobs ON jobs.id = tasks.job_id
      WHERE tasks.stream_id = stream
        AND tasks.state = 'ready'
        AND jobs.state = 'running'
      ORDER BY created_at ASC
      LIMIT batch_size
      FOR UPDATE SKIP LOCKED
    ),
    update_tasks AS (
      UPDATE tasks
      SET state = 'running',
          started_at = now()
      FROM selected_tasks
      WHERE tasks.job_id = selected_tasks.job_id
        AND tasks.task_id = selected_tasks.task_id
      RETURNING tasks.job_id, tasks.task_id, tasks.task_def, tasks.max_retries
    )
    SELECT update_tasks.job_id, update_tasks.task_id, update_tasks.task_def, '[]'::jsonb as prereqs, update_tasks.max_retries
    FROM update_tasks;
  END IF;
END;
$$ LANGUAGE plpgsql;

-- Create function to fail tasks from failed jobs
-- This prevents workers from wasting resources processing orphaned tasks
CREATE OR REPLACE FUNCTION fail_tasks_from_failed_jobs()
RETURNS INTEGER as $$
DECLARE
  failed_count INTEGER;
BEGIN
  WITH updated AS (
    UPDATE tasks
    SET state = 'failed',
        error = 'Job failed',
        updated_at = now()
    WHERE job_id IN (SELECT id FROM jobs WHERE state = 'failed')
      AND state NOT IN ('done', 'failed')
    RETURNING job_id, task_id
  )
  SELECT COUNT(*) INTO failed_count FROM updated;

  RETURN failed_count;
END;
$$ LANGUAGE plpgsql;
