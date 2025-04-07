CREATE OR REPLACE FUNCTION request_work_block(in_worker_type TEXT, batch_size INTEGER)
  RETURNS TABLE (job_id UUID, task_id TEXT, task_def jsonb, prereqs jsonb, max_retries INTEGER) as $$
DECLARE
  stream UUID;
  cursor_pos INTEGER := 0;
BEGIN
  -- Use SKIP LOCKED when selecting the stream to prevent stream-level contention
  SELECT id INTO stream
  FROM streams
  WHERE streams.worker_type = in_worker_type
  ORDER BY priority
  LIMIT 1
  FOR UPDATE SKIP LOCKED;

  IF stream IS NOT NULL THEN
    -- Use a CTE to atomically select and update in one operation
    -- Get consecutive tasks from the same job, in ascending order by index
    RETURN QUERY
    WITH selected_tasks AS (
      SELECT tasks.job_id, tasks.task_id, tasks.task_def, tasks.max_retries,
             ROW_NUMBER() OVER(PARTITION BY tasks.job_id ORDER BY created_at ASC) as row_num
      FROM tasks
      WHERE tasks.stream_id = stream
        AND tasks.state = 'ready'
      ORDER BY tasks.job_id, created_at ASC
      LIMIT batch_size
      FOR UPDATE SKIP LOCKED
    ),
    group_tasks AS (
      -- Group consecutive tasks by their row position
      SELECT *, row_num - cursor_pos as group_id
      FROM selected_tasks
      WHERE job_id = (SELECT job_id FROM selected_tasks LIMIT 1)
    ),
    update_tasks AS (
      UPDATE tasks
      SET state = 'running',
          started_at = now()
      FROM group_tasks
      WHERE tasks.job_id = group_tasks.job_id
        AND tasks.task_id = group_tasks.task_id
      RETURNING group_tasks.job_id, group_tasks.task_id, group_tasks.task_def, group_tasks.max_retries
    )
    SELECT ut.job_id, ut.task_id, ut.task_def, '[]'::jsonb as prereqs, ut.max_retries
    FROM update_tasks ut;
  END IF;
END;
$$ LANGUAGE plpgsql;
