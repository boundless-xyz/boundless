CREATE OR REPLACE FUNCTION fix_stuck_pending_tasks()
RETURNS INTEGER AS $$
DECLARE
    total_updated INTEGER := 0;
BEGIN
    WITH stuck_tasks AS (
        SELECT
            t.job_id,
            t.task_id,
            t.waiting_on,
            COUNT(td.pre_task_id) as actual_deps,
            COUNT(CASE WHEN prereq.state = 'done' THEN 1 END) as completed_deps
        FROM tasks t
        LEFT JOIN task_deps td ON td.job_id = t.job_id AND td.post_task_id = t.task_id
        LEFT JOIN tasks prereq ON prereq.job_id = td.job_id AND prereq.task_id = td.pre_task_id
        WHERE t.state = 'pending'
        GROUP BY t.job_id, t.task_id, t.waiting_on
        HAVING (
            -- Case 1: No dependencies at all (should be ready)
            COUNT(td.pre_task_id) = 0
            OR
            -- Case 2: All dependencies are complete
            COUNT(td.pre_task_id) = COUNT(CASE WHEN prereq.state = 'done' THEN 1 END)
        )
    )
    UPDATE tasks
    SET
        state = 'ready',
        waiting_on = 0,
        updated_at = NOW()
    FROM stuck_tasks st
    WHERE tasks.job_id = st.job_id
      AND tasks.task_id = st.task_id
      AND tasks.state = 'pending';

    GET DIAGNOSTICS total_updated = ROW_COUNT;

    RETURN total_updated;
END;
$$ LANGUAGE plpgsql;

-- Function to check for stuck tasks without updating them (for monitoring)
CREATE OR REPLACE FUNCTION check_stuck_pending_tasks()
RETURNS TABLE (
    job_id UUID,
    task_id TEXT,
    waiting_on INTEGER,
    actual_deps INTEGER,
    completed_deps INTEGER,
    should_be_ready BOOLEAN
) AS $$
BEGIN
    RETURN QUERY
    SELECT
        task.job_id,
        task.task_id,
        task.waiting_on,
        COUNT(task_dep.pre_task_id)::INTEGER as actual_deps,
        COUNT(CASE WHEN prereq.state = 'done' THEN 1 END)::INTEGER as completed_deps,
        (
            -- Case 1: No dependencies at all (should be ready)
            COUNT(task_dep.pre_task_id) = 0
            OR
            -- Case 2: All dependencies are complete
            COUNT(task_dep.pre_task_id) = COUNT(CASE WHEN prereq.state = 'done' THEN 1 END)
        ) as should_be_ready
    FROM tasks task
    LEFT JOIN task_deps task_dep ON task_dep.job_id = task.job_id AND task_dep.post_task_id = task.task_id
    LEFT JOIN tasks prereq ON prereq.job_id = task_dep.job_id AND prereq.task_id = task_dep.pre_task_id
    WHERE task.state = 'pending'
    GROUP BY task.job_id, task.task_id, task.waiting_on
    HAVING (
        -- Case 1: No dependencies at all (should be ready)
        COUNT(task_dep.pre_task_id) = 0
        OR
        -- Case 2: All dependencies are complete
        COUNT(task_dep.pre_task_id) = COUNT(CASE WHEN prereq.state = 'done' THEN 1 END)
    );
END;
$$ LANGUAGE plpgsql;


-- clear out completed jobs
CREATE OR REPLACE FUNCTION clear_completed_jobs()
RETURNS INTEGER AS $$
DECLARE
    total_cleared INTEGER := 0;
BEGIN
    DELETE FROM task_deps WHERE job_id IN (SELECT id FROM jobs WHERE state = 'done');
    DELETE FROM tasks WHERE job_id IN (SELECT id FROM jobs WHERE state = 'done');
    DELETE FROM jobs WHERE state = 'done';
    GET DIAGNOSTICS total_cleared = ROW_COUNT;
    RETURN total_cleared;
END;
$$ LANGUAGE plpgsql;
