CREATE OR REPLACE FUNCTION refresh_stream_counters()
RETURNS void AS $$
BEGIN
  WITH candidates AS (
    -- Only lock rows not currently held by request_work()
    SELECT id FROM streams FOR UPDATE SKIP LOCKED
  ),
  counts AS (
    SELECT stream_id,
           COUNT(*) FILTER (WHERE state = 'ready')::INTEGER   AS ready_cnt,
           COUNT(*) FILTER (WHERE state = 'running')::INTEGER AS running_cnt
    FROM tasks
    WHERE state IN ('ready', 'running')
    GROUP BY stream_id
  )
  UPDATE streams s
  SET ready   = COALESCE(counts.ready_cnt, 0),
      running = COALESCE(counts.running_cnt, 0)
  FROM candidates
  LEFT JOIN counts ON counts.stream_id = candidates.id
  WHERE s.id = candidates.id
    AND (s.ready   != COALESCE(counts.ready_cnt, 0)
      OR s.running != COALESCE(counts.running_cnt, 0));
END;
$$ LANGUAGE plpgsql;
