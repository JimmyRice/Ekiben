# kippu-store

Storage ports for Kippu: the traits a database adapter implements, the **consistency
contract** it must honour, and a conformance test suite (feature `conformance`) that proves it.

Kippu is not tied to one database. Instead of assuming "whatever PostgreSQL does", this crate
states exactly which guarantees the business logic relies on — and every adapter runs the same
suite to show it provides them:

| Guarantee | Relied on for |
|---|---|
| A [`StoreTx`] is atomic: all of its writes commit, or none do. | Payment + issuance happen together. |
| [`InventoryTx::try_hold`] is linearizable and all-or-nothing; `held + sold ≤ capacity` always. | No overselling. |
| Unique keys reject duplicates and report [`Insertion::Existing`]. | Idempotent purchases and attestations. |
| Claims and conditional updates succeed for exactly one caller. | Safe workers on any number of instances. |

Business logic lives once, in `kippu-core`; adapters only implement these primitives.
