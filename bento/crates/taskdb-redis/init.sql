-- Initialize PostgreSQL for comparison testing
-- This creates the same schema as the original TaskDB

CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE TYPE job_state AS ENUM (
  'running',
  'done',
  'failed'
);

CREATE TYPE task_state AS ENUM (
  'pending',
  'ready',
  'running',
  'done',
  'failed'
);

CREATE TABLE streams (
  id UUID NOT NULL DEFAULT uuid_generate_v4() PRIMARY KEY,
  worker_type TEXT NOT NULL,
  reserved INTEGER NOT NULL,
  be_mult REAL NOT NULL,
  user_id TEXT NOT NULL,
  running INTEGER NOT NULL DEFAULT(0),
  ready INTEGER NOT NULL DEFAULT(0),
  priority REAL NOT NULL GENERATED ALWAYS AS (
    CASE WHEN ready=0 THEN 'infinity'
         WHEN running >= reserved THEN (running - reserved) / be_mult
         ELSE (running - reserved) * 1.0 / reserved
    END) STORED
);

CREATE TABLE jobs (
  id UUID NOT NULL DEFAULT uuid_generate_v4() PRIMARY KEY,
  state job_state NOT NULL,
  error TEXT,
  user_id TEXT NOT NULL,
  reported BOOLEAN NOT NULL DEFAULT(FALSE)
);

CREATE TABLE tasks (
  job_id UUID NOT NULL,
  task_id TEXT NOT NULL,
  stream_id UUID NOT NULL,
  task_def jsonb NOT NULL,
  prerequisites jsonb NOT NULL,
  state task_state NOT NULL,
  created_at TIMESTAMP NOT NULL DEFAULT NOW(),
  started_at TIMESTAMP,
  updated_at TIMESTAMP,
  waiting_on INTEGER NOT NULL,
  progress REAL NOT NULL DEFAULT 0.0,
  retries INTEGER NOT NULL DEFAULT 0,
  max_retries INTEGER NOT NULL DEFAULT 0,
  timeout_secs INTEGER NOT NULL,
  output jsonb,
  error TEXT,
  PRIMARY KEY (job_id, task_id),
  FOREIGN KEY (job_id) REFERENCES jobs(id),
  FOREIGN KEY (stream_id) REFERENCES streams(id)
);

CREATE TABLE task_deps (
  job_id UUID NOT NULL,
  pre_task_id TEXT NOT NULL,
  post_task_id TEXT NOT NULL,
  FOREIGN KEY (job_id) REFERENCES jobs(id),
  FOREIGN KEY (job_id, pre_task_id) REFERENCES tasks(job_id, task_id),
  FOREIGN KEY (job_id, post_task_id) REFERENCES tasks(job_id, task_id)
);

-- Create indexes
CREATE INDEX streams_by_priority ON streams USING btree (worker_type, priority);
CREATE INDEX tasks_by_stream ON tasks using btree (state, stream_id, created_at);
CREATE INDEX tasks_state ON tasks USING btree (job_id, state);
CREATE INDEX task_deps_idx ON task_deps USING btree (job_id, pre_task_id, post_task_id);
