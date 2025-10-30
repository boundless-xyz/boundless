-- clear out completed jobs
CREATE OR REPLACE FUNCTION clear_completed_jobs()
RETURNS INTEGER AS $$
DECLARE
    total_cleared INTEGER := 0;
BEGIN
    -- Store the count before deletion
    SELECT COUNT(*) INTO total_cleared FROM jobs WHERE state = 'done';

    -- Delete in correct order (respecting foreign keys)
    -- First delete task_deps (references tasks)
    DELETE FROM task_deps WHERE job_id IN (SELECT id FROM jobs WHERE state = 'done');

    -- Then delete tasks (references streams)
    DELETE FROM tasks WHERE job_id IN (SELECT id FROM jobs WHERE state = 'done');

    -- Delete orphaned streams (streams that are no longer referenced by any tasks)
    DELETE FROM streams WHERE id NOT IN (SELECT DISTINCT stream_id FROM tasks);

    -- Finally delete jobs
    DELETE FROM jobs WHERE state = 'done';

    RETURN total_cleared;
END;
$$ LANGUAGE plpgsql;
