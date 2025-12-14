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
  -- Migration changes this ordering to prioritize workers with higher reserved capacity in the case
  -- of a tie. This allows single GPU worker clusters to prioritize work.
  ORDER BY priority, reserved DESC
  LIMIT 1
  FOR UPDATE SKIP LOCKED;

  IF stream IS NOT NULL THEN
    WITH selected_task AS (
      SELECT tasks.job_id, tasks.task_id, tasks.task_def, tasks.max_retries
      FROM tasks
      WHERE tasks.stream_id = stream
        AND tasks.state = 'ready'
      ORDER BY created_at ASC
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
