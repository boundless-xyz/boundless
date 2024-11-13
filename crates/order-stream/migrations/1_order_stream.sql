CREATE TABLE active_orders (
    id SERIAL PRIMARY KEY,
    data JSONB
);

CREATE TABLE brokers (
    addr BYTEA PRIMARY KEY
)