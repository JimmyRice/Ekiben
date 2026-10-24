-- A sale's purchase queue is served by one transaction at a time: a worker takes the sale's
-- row here with FOR UPDATE SKIP LOCKED and reads the oldest queued requests under it. However
-- many workers run on however many instances, each sale is served in arrival order, and a
-- worker that dies holds nothing once its transaction is gone. Sales created by instances
-- that predate this table get their row when a worker first takes it.
--
-- purchase_requests.lease_until keeps its name, so instances still running the previous
-- release work on during an upgrade, but no longer leases a request to a worker: it postpones
-- a request that failed on its own.

CREATE TABLE purchase_queues (
    sale_id BINARY(16) PRIMARY KEY,
    FOREIGN KEY (sale_id) REFERENCES sales (id)
);
INSERT INTO purchase_queues (sale_id) SELECT id FROM sales;

-- The queue of one sale in arrival order (InnoDB appends the id); also counts its backlog and
-- serves the sale_id foreign key, so it comes before the index it replaces goes.
CREATE INDEX purchase_requests_sale_queue ON purchase_requests (sale_id, status, created_at);
DROP INDEX purchase_requests_backlog ON purchase_requests;
