-- Purchase workers read one sale's queued requests in arrival order. SQLite's transactions
-- never overlap, so it needs no queue rows to serve each sale in order, only the index.
--
-- purchase_requests.lease_until keeps its name but no longer leases a request to a worker: it
-- postpones a request that failed on its own.

-- The queue of one sale in arrival order; also counts its backlog.
CREATE INDEX purchase_requests_sale_queue ON purchase_requests (sale_id, created_at, id)
    WHERE status = 'queued';
DROP INDEX purchase_requests_backlog;
