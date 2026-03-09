-- Replace per-row trigger with a bulk refresh function called by aux workers on a timer.
-- This avoids trigger overhead and the chicken-and-egg of who schedules the refresh.

CREATE OR REPLACE FUNCTION refresh_stream_counters()
RETURNS void AS $$
BEGIN
  UPDATE streams SET ready = 0, running = 0;
  UPDATE streams s SET ready = c.ready_cnt, running = c.running_cnt
  FROM (
    SELECT stream_id,
           COUNT(*) FILTER (WHERE state = 'ready')::INTEGER AS ready_cnt,
           COUNT(*) FILTER (WHERE state = 'running')::INTEGER AS running_cnt
    FROM tasks
    GROUP BY stream_id
  ) c
  WHERE s.id = c.stream_id;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS maint_streams_trigger ON tasks;
DROP FUNCTION IF EXISTS maint_streams();
