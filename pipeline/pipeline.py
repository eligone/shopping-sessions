#!/usr/bin/env python3
"""
Shopping Sessions: DuckDB pipeline.

Reads  $DATA_DIR    (default /data)
Writes $OUTPUT_DIR  (default /output): user_daily/ and campaign_daily/ as Parquet
Temp   $SCRATCH_DIR (default /scratch)

Stages (see rules.md):
  1. Raw CSV parsing by hand (DuckDB's CSV reader treats \r as a line break and
     handles quotes/malformed lines differently from the rules).
  2. Linked IDs -> persons (union-find in NumPy, named by the byte-wise smallest ID).
  3. Events: dedup on event_id (earliest ingest_ts) -> validate -> drop late
     -> sessionize per person (gap > 30 min) -> day of session start.
  4. Clicks: dedup -> validate -> credit (ASOF join) and assists (merged intervals).
  5. Aggregate and write both tables.
"""
import glob
import json
import os
from datetime import datetime, timezone

import duckdb
import numpy as np

DATA = os.environ.get("DATA_DIR", "/data")
OUT = os.environ.get("OUTPUT_DIR", "/output")
SCRATCH = os.environ.get("SCRATCH_DIR", "/scratch")
NCPU = int(os.environ.get("NCPU", os.cpu_count() or 1))
MEM_GB = int(os.environ.get("MEM_GB", "8"))

MIN_MS = 60_000
DAY_MS = 86_400_000
GAP_MS = 30 * MIN_MS
WEEK_MS = 7 * DAY_MS
MAX_PRICE = 10_000_000

con = duckdb.connect()  # in-memory database
con.execute(f"SET threads = {NCPU}")
con.execute(f"SET memory_limit = '{max(1, int(MEM_GB * 0.8))}GB'")
con.execute(f"SET temp_directory = '{os.path.join(SCRATCH, 'duckdb_tmp')}'")
con.execute("SET preserve_insertion_order = false")


def sql_str(s):
    return "'" + s.replace("'", "''") + "'"


def f1_to_ms(s):
    return int(datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc).timestamp() * 1000)


with open(os.path.join(DATA, "manifest.json")) as f:
    manifest = json.load(f)
WS = f1_to_ms(manifest["window_start"])
WE = f1_to_ms(manifest["window_end"])
print("window:", manifest["window_start"], "to", manifest["window_end"])

# ---------------------------------------------------------------------------
# Timestamp parsing: F1 / F2 / F3 -> epoch ms, NULL when bad.
# CASE is evaluated lazily per row in DuckDB, so the casts only run on rows
# that already matched the shape regex.
# ---------------------------------------------------------------------------
TS_SHAPE = "[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(Z|[+-][0-9]{2}:[0-9]{2})"
con.execute(f"""
CREATE MACRO parse_ts(s) AS CASE
  WHEN s IS NULL THEN NULL
  WHEN regexp_full_match(s, '[0-9]+') THEN
    CASE WHEN length(ltrim(s, '0')) > 15 THEN NULL          -- far outside any window
         WHEN ltrim(s, '0') = '' THEN 0::BIGINT
         ELSE CAST(ltrim(s, '0') AS BIGINT) END
  WHEN regexp_full_match(s, '{TS_SHAPE}') THEN
    CASE WHEN length(s) = 25 AND (CAST(substr(s, 21, 2) AS INT) > 23
                                  OR CAST(substr(s, 24, 2) AS INT) > 59) THEN NULL
         -- strptime rejects impossible dates, hour 24 and second 60
         ELSE epoch_ms(try_strptime(substr(s, 1, 19), '%Y-%m-%dT%H:%M:%S'))
              - CASE WHEN length(s) = 25
                     THEN (CASE WHEN substr(s, 20, 1) = '+' THEN 1 ELSE -1 END)
                          * (CAST(substr(s, 21, 2) AS BIGINT) * 60 + CAST(substr(s, 24, 2) AS BIGINT))
                          * {MIN_MS}
                     ELSE 0 END
    END
  ELSE NULL
END
""")

# ---------------------------------------------------------------------------
# 1. Raw CSV parsing
# A field is either "..." (no quote/comma inside) or bare (no quote/comma).
# A line is valid iff it is exactly ncols such fields. Quotes are stripped,
# an empty value is NULL. \r is an ordinary character.
# ---------------------------------------------------------------------------
FIELD = '(?:"[^",]*"|[^",]*)'


def load_csv_dir(table, folder, columns):
    files = sorted(p for p in glob.glob(os.path.join(DATA, folder, "*.csv")) if os.path.isfile(p))
    n = len(columns)
    if not files:
        cols = ", ".join(f"NULL::VARCHAR AS {c}" for c in columns)
        con.execute(f"CREATE TEMP TABLE {table} AS SELECT {cols} WHERE false")
        return
    file_list = "[" + ", ".join(sql_str(p) for p in files) + "]"
    line_re = FIELD + "(?:," + FIELD + "){" + str(n - 1) + "}"
    cols = ", ".join(f"nullif(replace(f[{i + 1}], '\"', ''), '') AS {c}" for i, c in enumerate(columns))
    con.execute(f"""
        CREATE TEMP TABLE {table} AS
        WITH raw AS (
            SELECT unnest(string_split(content, chr(10))[2:]) AS line   -- [2:] skips the header
            FROM read_text({file_list})
        ),
        ok AS (
            SELECT string_split(line, ',') AS f FROM raw
            WHERE regexp_full_match(line, {sql_str(line_re)})
        )
        SELECT {cols} FROM ok
    """)


load_csv_dir("ev_raw", "events", ["event_id", "user_id", "ts", "ingest_ts", "event_type", "product_id", "price_cents"])
load_csv_dir("ck_raw", "clicks", ["click_id", "user_id", "campaign_id", "ts"])
load_csv_dir("ln_raw", "links", ["a", "b"])

# products.csv and campaigns.csv are clean, but run them through the same parser.
for table, name, cols in (("products", "products.csv", ["product_id", "category"]),
                          ("campaigns", "campaigns.csv", ["campaign_id", "channel"])):
    path = os.path.join(DATA, name)
    con.execute(f"""
        CREATE TEMP TABLE {table} AS
        WITH raw AS (SELECT unnest(string_split(content, chr(10))[2:]) AS line FROM read_text({sql_str(path)})),
        ok AS (SELECT string_split(line, ',') AS f FROM raw
               WHERE regexp_full_match(line, {sql_str(FIELD + ',' + FIELD)}))
        SELECT nullif(replace(f[1], '"', ''), '') AS {cols[0]},
               nullif(replace(f[2], '"', ''), '') AS {cols[1]}
        FROM ok WHERE f[1] IS NOT NULL
    """)

# ---------------------------------------------------------------------------
# 2. Linked IDs -> persons
# IDs get dense indexes in byte-wise order, so the component minimum index is
# the byte-wise smallest ID, i.e. the person's name.
# ---------------------------------------------------------------------------
con.execute("""
    CREATE TEMP TABLE link_ids AS
    SELECT id, (row_number() OVER (ORDER BY id) - 1)::BIGINT AS idx
    FROM (SELECT a AS id FROM ln_raw WHERE a IS NOT NULL AND b IS NOT NULL
          UNION SELECT b FROM ln_raw WHERE a IS NOT NULL AND b IS NOT NULL)
""")
edges = con.execute("""
    SELECT DISTINCT ia.idx AS a, ib.idx AS b
    FROM ln_raw l JOIN link_ids ia ON l.a = ia.id JOIN link_ids ib ON l.b = ib.id
    WHERE ia.idx <> ib.idx
""").fetchnumpy()
n_ids = con.execute("SELECT count(*) FROM link_ids").fetchone()[0]

label = np.arange(n_ids, dtype=np.int64)
ea = np.asarray(edges["a"], dtype=np.int64)
eb = np.asarray(edges["b"], dtype=np.int64)
while len(ea):
    # hook: each endpoint takes the smaller label of the two
    m = np.minimum(label[ea], label[eb])
    new = label.copy()
    np.minimum.at(new, ea, m)
    np.minimum.at(new, eb, m)
    # shortcut: pointer jumping until every node points at a root
    while True:
        jumped = new[new]
        if np.array_equal(jumped, new):
            break
        new = jumped
    if np.array_equal(new, label):
        break
    label = new

person_lab = {"idx": np.arange(n_ids, dtype=np.int64), "lab": label}
con.execute("""
    CREATE TEMP TABLE person_map AS
    SELECT i.id AS user_id, r.id AS person
    FROM person_lab p JOIN link_ids i ON i.idx = p.idx JOIN link_ids r ON r.idx = p.lab
""")
print(f"links: {n_ids} linked ids, {len(np.unique(label))} persons")

# ---------------------------------------------------------------------------
# 3. Events
# ---------------------------------------------------------------------------
con.execute(f"""
    CREATE TEMP TABLE ev AS
    WITH dedup AS (                      -- dedup BEFORE validation: earliest ingest_ts wins
        SELECT * FROM (SELECT *, parse_ts(ingest_ts) AS ingest_ms FROM ev_raw WHERE event_id IS NOT NULL)
        QUALIFY row_number() OVER (PARTITION BY event_id ORDER BY ingest_ms) = 1
    ),
    parsed AS (
        SELECT *, parse_ts(ts) AS ts_ms FROM dedup
    ),
    valid AS (
        SELECT user_id, ts_ms, event_type, product_id,
               CASE WHEN event_type = 'purchase' THEN CAST(ltrim(price_cents, '0') AS BIGINT) ELSE 0 END AS price
        FROM parsed
        WHERE user_id IS NOT NULL
          AND ts_ms IS NOT NULL AND ts_ms >= {WS} AND ts_ms < {WE}
          AND event_type IN ('view', 'cart', 'purchase')
          AND product_id IS NOT NULL
          AND (event_type <> 'purchase' OR (
                price_cents IS NOT NULL
                AND regexp_full_match(price_cents, '[0-9]+')
                AND length(ltrim(price_cents, '0')) BETWEEN 1 AND 8
                AND CAST(ltrim(price_cents, '0') AS BIGINT) BETWEEN 1 AND {MAX_PRICE}))
          AND ingest_ms - ts_ms <= {DAY_MS}       -- late events: strictly over 24 h is dropped
    )
    SELECT coalesce(pm.person, v.user_id) AS person, v.ts_ms, v.event_type, v.price,
           coalesce(p.category, 'unknown') AS category
    FROM valid v
    LEFT JOIN person_map pm ON pm.user_id = v.user_id
    LEFT JOIN products p ON p.product_id = v.product_id
""")

# Sessionize: a new session starts when the gap to the previous event is > 30 min.
# Equal timestamps have gap 0, so they always share a session.
con.execute(f"""
    CREATE TEMP TABLE ev_s AS
    WITH g AS (
        SELECT *, CASE WHEN ts_ms - lag(ts_ms) OVER w > {GAP_MS} OR lag(ts_ms) OVER w IS NULL
                       THEN 1 ELSE 0 END AS is_new
        FROM ev WINDOW w AS (PARTITION BY person ORDER BY ts_ms)
    )
    SELECT *, sum(is_new) OVER (PARTITION BY person ORDER BY ts_ms
                                ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS sess_no
    FROM g
""")

con.execute("""
    CREATE TEMP TABLE sessions AS
    SELECT person, sess_no, min(ts_ms) AS start_ms,
           CAST(make_timestamp(min(ts_ms) * 1000) AS DATE) AS day,
           count(*) AS n_events,
           sum(CASE WHEN event_type = 'purchase' THEN price ELSE 0 END) AS revenue,
           bool_or(event_type = 'purchase') AS has_purchase
    FROM ev_s GROUP BY person, sess_no
""")

# ---------------------------------------------------------------------------
# 4. Clicks
# ---------------------------------------------------------------------------
con.execute(f"""
    CREATE TEMP TABLE clicks AS
    WITH d AS (
        SELECT click_id, any_value(user_id) AS user_id, any_value(campaign_id) AS campaign_id,
               any_value(ts) AS ts
        FROM ck_raw WHERE click_id IS NOT NULL GROUP BY click_id
    ),
    v AS (
        SELECT d.click_id, d.user_id, d.campaign_id, parse_ts(d.ts) AS ts_ms
        FROM d SEMI JOIN campaigns c ON c.campaign_id = d.campaign_id
        WHERE d.user_id IS NOT NULL
    )
    SELECT coalesce(pm.person, v.user_id) AS person, v.click_id, v.campaign_id, v.ts_ms
    FROM v LEFT JOIN person_map pm ON pm.user_id = v.user_id
    WHERE v.ts_ms IS NOT NULL AND v.ts_ms >= {WS - WEEK_MS} AND v.ts_ms < {WE}
""")

# Credit: latest click at or before the session start (ties: smallest click_id),
# no more than 7 days before it. Collapse ties first, then an ASOF join.
con.execute(f"""
    CREATE TEMP TABLE credit AS
    WITH best AS (
        SELECT person, ts_ms, arg_min(campaign_id, click_id) AS campaign_id
        FROM clicks GROUP BY person, ts_ms
    )
    SELECT s.person, s.sess_no, s.day, s.revenue, s.has_purchase,
           CASE WHEN b.ts_ms IS NOT NULL AND s.start_ms - b.ts_ms <= {WEEK_MS}
                THEN b.campaign_id ELSE 'organic' END AS campaign_id
    FROM sessions s ASOF LEFT JOIN best b
      ON s.person = b.person AND s.start_ms >= b.ts_ms
""")

# Assists: a campaign assists a session if one of its clicks falls in
# [start - 7d, start]  <=>  start in [click, click + 7d]. Merge each
# (person, campaign)'s click intervals into disjoint ranges, then range-join.
con.execute(f"""
    CREATE TEMP TABLE assists AS
    WITH c AS (
        SELECT person, campaign_id, ts_ms,
               CASE WHEN ts_ms - lag(ts_ms) OVER w > {WEEK_MS} OR lag(ts_ms) OVER w IS NULL
                    THEN 1 ELSE 0 END AS is_new
        FROM clicks WINDOW w AS (PARTITION BY person, campaign_id ORDER BY ts_ms)
    ),
    g AS (
        SELECT *, sum(is_new) OVER (PARTITION BY person, campaign_id ORDER BY ts_ms
                                    ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS grp
        FROM c
    ),
    iv AS (
        SELECT person, campaign_id, min(ts_ms) AS lo, max(ts_ms) + {WEEK_MS} AS hi
        FROM g GROUP BY person, campaign_id, grp
    )
    SELECT iv.campaign_id, s.day, count(*) AS assisted_sessions
    FROM sessions s JOIN iv ON s.person = iv.person AND s.start_ms >= iv.lo AND s.start_ms <= iv.hi
    GROUP BY iv.campaign_id, s.day
""")

# ---------------------------------------------------------------------------
# 5. Outputs
# ---------------------------------------------------------------------------
os.makedirs(os.path.join(OUT, "user_daily"), exist_ok=True)
os.makedirs(os.path.join(OUT, "campaign_daily"), exist_ok=True)

con.execute(f"""
    COPY (
        WITH cat AS (
            SELECT s.person, s.day, e.category, sum(e.price) AS rev
            FROM ev_s e JOIN sessions s USING (person, sess_no)
            WHERE e.event_type = 'purchase'
            GROUP BY s.person, s.day, e.category
        ),
        top AS (
            SELECT person, day, category AS top_category FROM cat
            QUALIFY row_number() OVER (PARTITION BY person, day ORDER BY rev DESC, category ASC) = 1
        ),
        agg AS (
            SELECT person, day, count(*)::BIGINT AS sessions, sum(n_events)::BIGINT AS events,
                   sum(revenue)::BIGINT AS revenue_cents
            FROM sessions GROUP BY person, day
        )
        SELECT a.person AS user_id, a.day, a.sessions, a.events, a.revenue_cents, t.top_category
        FROM agg a LEFT JOIN top t USING (person, day)
    ) TO '{os.path.join(OUT, "user_daily", "part-0.parquet")}' (FORMAT parquet)
""")

con.execute(f"""
    COPY (
        WITH cr AS (
            SELECT campaign_id, day, count(*)::BIGINT AS sessions,
                   count(*) FILTER (WHERE has_purchase)::BIGINT AS purchasing_sessions,
                   sum(revenue)::BIGINT AS revenue_cents
            FROM credit GROUP BY campaign_id, day
        ),
        j AS (
            SELECT coalesce(cr.campaign_id, a.campaign_id) AS campaign_id,
                   coalesce(cr.day, a.day) AS day,
                   coalesce(cr.sessions, 0)::BIGINT AS sessions,
                   coalesce(cr.purchasing_sessions, 0)::BIGINT AS purchasing_sessions,
                   coalesce(cr.revenue_cents, 0)::BIGINT AS revenue_cents,
                   coalesce(a.assisted_sessions, 0)::BIGINT AS assisted_sessions
            FROM cr FULL OUTER JOIN assists a ON cr.campaign_id = a.campaign_id AND cr.day = a.day
        )
        SELECT j.campaign_id,
               CASE WHEN j.campaign_id = 'organic' THEN 'organic' ELSE c.channel END AS channel,
               j.day, j.sessions, j.purchasing_sessions, j.revenue_cents, j.assisted_sessions
        FROM j LEFT JOIN campaigns c ON c.campaign_id = j.campaign_id
    ) TO '{os.path.join(OUT, "campaign_daily", "part-0.parquet")}' (FORMAT parquet)
""")

n_ud = con.execute("SELECT count(*) FROM sessions").fetchone()[0]
print(f"done: {n_ud} sessions")
