#!/usr/bin/env bash
source .env-compose

echo "what job are you querying?"
read -r JOB

PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" "${POSTGRES_DB}" -c "SELECT count(task_id) as jobs_count FROM tasks WHERE job_id = '${JOB}';"

PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" "${POSTGRES_DB}" -c "SELECT COUNT(*) as remaining_jobs FROM tasks WHERE job_id = '${JOB}' AND state != 'done';"

echo 'task times:'
PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" "${POSTGRES_DB}" -c "SELECT task_id, jsonb_object_keys(task_def) as task_type, state, EXTRACT(EPOCH FROM updated_at) - EXTRACT(EPOCH FROM started_at) as wall_time, started_at FROM tasks WHERE job_id = '${JOB}' ORDER BY started_at limit 10;"

echo "Effective Hz:"
PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" "${POSTGRES_DB}" -c "SELECT CAST(res1.total_cycles AS decimal)  / res2.elapsed_sec AS hz, res1.total_cycles, res2.elapsed_sec FROM (SELECT output->'total_cycles' total_cycles FROM tasks WHERE task_id = 'init' AND job_id = '${JOB}') res1, (SELECT EXTRACT(EPOCH FROM (MAX(updated_at) - MIN(started_at))) AS elapsed_sec FROM tasks WHERE job_id = '${JOB}') res2;"
