#!/usr/bin/env python3
"""Off-chain proving-cost numbers for the assessor cost analysis.

Run from a terminal with AWS access (psql is not required):

    uv run --with 'psycopg[binary]' --with duckdb --with boto3 \
        contracts/benchmarks/run-telemetry.py prod_base

Tries LIVE Redshift first (needs the corporate VPN — the cluster security
group is IP-allowlisted). Falls back to the S3 Parquet ARCHIVE (data through
2026-04-24, no VPN needed; proving costs are stable week-to-week, so it is
representative). Reads creds from ./network_secrets.toml.

Paste the output back and it gets folded into
contracts/benchmarks/assessor-cost-analysis.md (Part 4 + Part 5).
"""
import sys, tomllib

ENV = sys.argv[1] if len(sys.argv) > 1 else "prod_base"
BUCKETS = {
    "prod_taiko": "telemetry-snapshot-prod-167000-04-24-26",
    "prod_base": "telemetry-snapshot-prod-8453-04-24-26",
    "prod_base_sepolia": "telemetry-snapshot-prod-84532-04-24-26",
    "staging_taiko": "telemetry-snapshot-staging-167000-04-24-26",
    "staging_base_sepolia": "telemetry-snapshot-staging-84532-04-24-26",
}
d = tomllib.load(open("network_secrets.toml", "rb"))


def show(title, cols, rows):
    print(f"\n== {title} ==")
    print(" | ".join(cols))
    for r in rows:
        print(" | ".join("" if x is None else str(x) for x in r))


def try_live():
    import psycopg
    t = d["environments"][ENV]["telemetry"]
    url = f"postgres://readonly:{t['readonly_password']}@{t['db_url']}"
    win = "completed_at >= GETDATE() - INTERVAL '30 days'"
    out = []
    with psycopg.connect(url, connect_timeout=20) as conn:
        for title, q in queries("GETDATE", win):
            with conn.cursor() as cur:
                cur.execute(q)
                out.append((title, [c.name for c in cur.description], cur.fetchall()))
    return out


def try_archive():
    import duckdb
    aws = d["aws"]["prod" if ENV.startswith("prod") else "staging"]
    bucket = BUCKETS[ENV]
    con = duckdb.connect()
    con.execute("INSTALL httpfs; LOAD httpfs;")
    con.execute("SET s3_region='us-west-2';")
    con.execute(f"SET s3_access_key_id='{aws['access_key_id']}';")
    con.execute(f"SET s3_secret_access_key='{aws['secret_access_key']}';")
    glob = f"s3://{bucket}/*/request_completions/*.parquet"
    win = ("completed_at BETWEEN TIMESTAMP '2026-03-25 00:00:00+00' "
           "AND TIMESTAMP '2026-04-25 00:00:00+00'")
    out = []
    for title, q in queries("ARCHIVE", win, glob):
        cur = con.execute(q)
        out.append((title, [c[0] for c in cur.description], cur.fetchall()))
    return out


def queries(mode, win, glob=None):
    src = f"read_parquet('{glob}')" if mode == "ARCHIVE" else "telemetry.request_completions"
    base = f"{src} WHERE proof_type='Merkle' AND outcome='Fulfilled'"
    # percentile syntax differs: Redshift WITHIN GROUP can't be mixed; DuckDB QUANTILE_CONT can.
    if mode == "ARCHIVE":
        def stats(col):
            return (f"SELECT '{col}' AS component, COUNT(*) AS n, ROUND(AVG({col}),3) AS mean, "
                    f"ROUND(MEDIAN({col}),3) AS p50, ROUND(QUANTILE_CONT({col},0.9),3) AS p90, "
                    f"ROUND(QUANTILE_CONT({col},0.99),3) AS p99 FROM {base} AND {col} IS NOT NULL AND {win}")
        comp = "\nUNION ALL\n".join(stats(c) for c in
                                    ["assessor_proving_secs", "set_builder_proving_secs",
                                     "assessor_compression_proof_secs", "stark_proving_secs"])
    else:
        def one(col, p, lbl):
            return (f"SELECT '{col}_{lbl}' AS component, "
                    f"PERCENTILE_CONT({p}) WITHIN GROUP (ORDER BY {col}) AS secs "
                    f"FROM {base} AND {col} IS NOT NULL AND {win}")
        comp = "\nUNION ALL\n".join(
            one(c, p, lbl) for c in ["assessor_proving_secs", "set_builder_proving_secs",
                                     "assessor_compression_proof_secs", "stark_proving_secs"]
            for p, lbl in [(0.5, "p50"), (0.9, "p90"), (0.99, "p99")])
        comp = f"SELECT component, ROUND(secs,3) FROM (\n{comp}\n) t"

    return [
        ("Q1 path usage split (30d, fulfilled)",
         f"SELECT proof_type, COUNT(*) AS completions, COUNT(assessor_proving_secs) AS with_assessor, "
         f"COUNT(set_builder_proving_secs) AS with_set_builder FROM {src} "
         f"WHERE outcome='Fulfilled' AND {win} GROUP BY proof_type ORDER BY completions DESC"),
        ("Q2/Q3 Path B off-chain components (count/mean/percentiles)", comp),
        ("Q5 approx batch-size proxy (orders per aggregation)",
         f"SELECT orders_in_group, COUNT(*) AS num_batches FROM ("
         f"  SELECT broker_address, DATE_TRUNC('day', completed_at) AS day, "
         f"  ROUND(assessor_proving_secs,2) AS a, ROUND(set_builder_proving_secs,2) AS s, COUNT(*) AS orders_in_group"
         f"  FROM {base} AND assessor_proving_secs IS NOT NULL AND {win} GROUP BY 1,2,3,4"
         f") g GROUP BY orders_in_group ORDER BY orders_in_group"),
    ]


print(f"# env={ENV}")
try:
    print("# trying LIVE Redshift ...")
    res = try_live()
    print("# source: LIVE Redshift (last 30 days)")
except Exception as e:
    print(f"# live failed ({type(e).__name__}); falling back to S3 archive ...")
    res = try_archive()
    print("# source: S3 ARCHIVE (final 30 days through 2026-04-24)")
for title, cols, rows in res:
    show(title, cols, rows)
