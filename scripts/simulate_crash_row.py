#!/usr/bin/env python3
"""Write a `running` history row, standing in for a process killed mid-job.

The database state *is* the crash artifact: a row the app never got to finish.
`sweep` is expected to reconcile it to `interrupted`.
"""

import sqlite3
import sys
import time

db = sys.argv[1]
conn = sqlite3.connect(db)
conn.execute(
    "INSERT OR REPLACE INTO jobs"
    " (id, input, intent, output, status, started_at, ended_at, error_code, error_tail)"
    " VALUES ('behaviour-crash-row', '/tmp/crashed.mov',"
    " '{\"kind\":\"convert\",\"target\":\"MP4\"}', NULL, 'running', ?, NULL, NULL, NULL)",
    (int(time.time()),),
)
conn.commit()
print("inserted a running row into", db)
