-- Migration 10: Restore the per-row trigger on tasks to maintain streams.ready/running counters.
--
-- Migration 7 dropped this trigger in favor of a bulk refresh function (refresh_stream_counters).
-- Migration 9 added `WHERE streams.ready > 0` to request_work(), which requires the counters to
-- be accurate in real-time. Without the trigger, new ready tasks are invisible to request_work()
-- until the next periodic refresh.
--
-- The bulk refresh function (migration 8) is kept as a safety net for counter drift.

CREATE OR REPLACE FUNCTION maint_streams() RETURNS TRIGGER AS $$
DECLARE
    delta_ready INTEGER := 0;
    delta_running INTEGER := 0;
    sid UUID;
BEGIN
  IF TG_OP = 'INSERT' THEN
    sid := NEW.stream_id;
    IF NEW.state = 'ready' THEN delta_ready := 1; END IF;
    IF NEW.state = 'running' THEN delta_running := 1; END IF;
  ELSIF TG_OP = 'UPDATE' THEN
    sid := COALESCE(NEW.stream_id, OLD.stream_id);
    IF OLD.state = 'ready' THEN delta_ready := delta_ready - 1; END IF;
    IF OLD.state = 'running' THEN delta_running := delta_running - 1; END IF;
    IF NEW.state = 'ready' THEN delta_ready := delta_ready + 1; END IF;
    IF NEW.state = 'running' THEN delta_running := delta_running + 1; END IF;
  END IF;

  IF delta_ready != 0 OR delta_running != 0 THEN
    UPDATE streams SET
        ready = GREATEST(ready + delta_ready, 0),
        running = GREATEST(running + delta_running, 0)
    WHERE id = sid;
  END IF;

  RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER maint_streams_trigger
AFTER INSERT OR UPDATE ON tasks
FOR EACH ROW EXECUTE FUNCTION maint_streams();

-- Sync counters to current state so they're accurate before the trigger takes over.
SELECT refresh_stream_counters();
