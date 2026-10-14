-- An event's reservations and refunds, all of them or in one status, for its organizers.
CREATE INDEX reservations_by_event ON reservations (event_id, created_at, id);
CREATE INDEX reservations_by_event_status ON reservations (event_id, status, created_at, id);
CREATE INDEX refunds_by_event ON refunds (event_id, created_at, id);
CREATE INDEX refunds_by_event_status ON refunds (event_id, status, created_at, id);
