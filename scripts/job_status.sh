#!/usr/bin/env bash
source .env.broker

JOB=$1
if [ -z "$JOB" ]; then
    echo "Error: No job ID provided."
    exit 1
fi

PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" "${POSTGRES_DB}" -c "SELECT count(task_id) as jobs_count FROM tasks WHERE job_id = '${JOB}';"

PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" "${POSTGRES_DB}" -c "SELECT jsonb_object_keys(task_def) as task_type, COUNT(*) as remaining_count FROM tasks WHERE state != 'done' AND job_id = '${JOB}' GROUP BY jsonb_object_keys(task_def);"

echo 'task times:'
PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" "${POSTGRES_DB}" -c "SELECT jsonb_object_keys(task_def) as task_type, COUNT(*) as completed_count, AVG(EXTRACT(EPOCH FROM updated_at) - EXTRACT(EPOCH FROM started_at)) as avg_seconds, MIN(EXTRACT(EPOCH FROM updated_at) - EXTRACT(EPOCH FROM started_at)) as min_seconds, MAX(EXTRACT(EPOCH FROM updated_at) - EXTRACT(EPOCH FROM started_at)) as max_seconds FROM tasks WHERE job_id = '${JOB}' AND state = 'done' AND started_at IS NOT NULL AND updated_at IS NOT NULL GROUP BY jsonb_object_keys(task_def) ORDER BY avg_seconds DESC;"

echo 'task times (totals):'
PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" "${POSTGRES_DB}" -c "SELECT jsonb_object_keys(task_def) as task_type, COUNT(*) as completed_count, SUM(EXTRACT(EPOCH FROM updated_at) - EXTRACT(EPOCH FROM started_at)) as total_secs FROM tasks WHERE job_id = '${JOB}' AND state = 'done' AND started_at IS NOT NULL AND updated_at IS NOT NULL GROUP BY jsonb_object_keys(task_def) ORDER BY total_secs DESC;"

echo "Effective Hz:"
PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" "${POSTGRES_DB}" -c "SELECT CAST(res1.total_cycles AS decimal)  / res2.elapsed_sec AS hz, res1.total_cycles, res2.elapsed_sec FROM (SELECT output->'total_cycles' total_cycles FROM tasks WHERE task_id = 'init' AND job_id = '${JOB}') res1, (SELECT EXTRACT(EPOCH FROM (MAX(updated_at) - MIN(started_at))) AS elapsed_sec FROM tasks WHERE job_id = '${JOB}') res2;"
