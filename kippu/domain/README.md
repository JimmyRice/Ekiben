# kippu-domain

Kippu's pure domain model: identifiers, money, roles and the reservation state machine.

This crate performs no I/O. Every function that depends on the current time takes it as an
argument, so the whole domain is deterministic and trivially testable.
