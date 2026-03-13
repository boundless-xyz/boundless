-- Run this manually against the writer endpoint to create a read-only database user.
-- Replace <SET_PASSWORD_HERE> with a real password before running.

CREATE ROLE readonly WITH LOGIN PASSWORD '<SET_PASSWORD_HERE>';
GRANT CONNECT ON DATABASE indexerv19 TO readonly;
GRANT USAGE ON SCHEMA public TO readonly;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO readonly;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO readonly;
