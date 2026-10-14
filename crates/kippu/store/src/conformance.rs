//! The conformance suite: the consistency contract, as executable tests.
//!
//! Every adapter runs it against itself with one line:
//!
//! ```ignore
//! kippu_store::conformance_tests!(async {
//!     let store = MyStore::connect(...).await.unwrap();
//!     store.migrate().await.unwrap();
//!     kippu_store::conformance::Harness::new(store, ())
//! });
//! ```
//!
//! Each case builds its own records, so cases are independent of each other and of order.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    missing_docs,
    reason = "test code: a failed expectation is the test failing"
)]

use std::any::Any;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use kippu_domain::account::{Account, Identity, Organization, Role};
use kippu_domain::admission::AdmissionPolicy;
use kippu_domain::catalog::{
    Address, Event, EventStatus, EventSummary, MAX_EVENT_CONTENT_BYTES, Sale, TicketType,
};
use kippu_domain::denial::{Denial, DenialSubject, DeniedBecause, DeniedTicket};
use kippu_domain::image::{EventImage, ImageFormat};
use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::payment::{Environment, PaymentAttestation, PaymentDisposition};
use kippu_domain::purchase::{Basket, LineItem, PurchaseRequest, PurchaseStatus, RejectionReason};
use kippu_domain::refund::{Refund, RefundReason, RefundStatus};
use kippu_domain::reservation::{Reservation, ReservationStatus, ReservedItem};
use kippu_domain::ticket::{Ticket, TicketStatus};
use kippu_domain::validation::{CountryCode, Email, IdempotencyKey, ProviderName, Slug, Subject};
use kippu_domain::webhook::Webhook;
use kippu_domain::{
    AccountId, AttestorId, Currency, DenialId, Duration, EventId, ImageId, Money, OrganizationId,
    PurchaseRequestId, RefundId, ReservationId, SaleId, SessionId, TicketId, TicketTypeId,
    Timestamp, WebhookId,
};

use crate::{
    AccountFilter, AuditEntry, EventFilter, EventOrder, Holds, Insertion, Keyset, Lease, LineItems,
    PageRequest, Session, SessionRenewal, Store, StoreError, Unlink, WebhookRun,
};

/// A store under test, plus whatever must outlive it (e.g. a temporary directory).
pub struct Harness {
    pub store: Arc<dyn Store>,
    _guard: Box<dyn Any + Send>,
}

impl Harness {
    pub fn new(store: impl Store, guard: impl Any + Send) -> Self {
        Self {
            store: Arc::new(store),
            _guard: Box::new(guard),
        }
    }
}

/// Expands to one `#[tokio::test]` per conformance case. `$connect` is an expression that
/// evaluates to a future of a [`Harness`] with a migrated, empty store.
///
/// With `optional`, `$connect` evaluates to a future of an `Option<Harness>`, and each case is
/// skipped (it passes, saying so on stderr) when it yields `None` — for adapters that need a
/// database server the environment may not provide.
#[macro_export]
macro_rules! conformance_tests {
    (optional $connect:expr) => {
        $crate::conformance_tests!(@list (optional $connect));
    };
    ($connect:expr) => {
        $crate::conformance_tests!(@list (required $connect));
    };
    (@list $mode:tt) => {
        $crate::conformance_tests!(@cases $mode;
            migrations_are_idempotent,
            unique_keys_report_conflicts,
            stale_versions_conflict,
            purchase_requests_are_idempotent,
            inventory_never_oversells,
            holds_are_all_or_nothing,
            sale_inventory_reads_every_type_at_once,
            stock_locked_up_front_never_deadlocks,
            quotas_are_enforced_and_all_or_nothing,
            dropped_transactions_roll_back,
            a_purchase_queue_is_held_by_one_transaction,
            queued_requests_are_read_in_arrival_order_with_their_denials,
            attestations_are_recorded_once,
            one_reservation_per_purchase_request,
            waiting_room_positions_are_distinct,
            an_account_keeps_its_waiting_room_position,
            waiting_room_advances_once,
            outbox_is_transactional_and_ordered,
            outbox_readers_never_skip_late_commits,
            sessions_rotate_once,
            only_holding_reservations_are_overdue,
            capacity_cannot_drop_below_stock_in_use,
            accounts_may_lack_email_and_password,
            first_external_sign_ins_create_one_account,
            identities_link_to_one_account,
            the_last_sign_in_method_is_kept,
            webhooks_are_claimed_by_one_worker_when_due,
            webhook_settings_and_progress_are_separate,
            event_images_keep_their_order,
            event_content_is_kept_whole_and_left_out_of_listings,
            event_addresses_round_trip,
            event_listings_resume_exactly_in_every_order,
            event_listings_filter,
            account_listings_resume_exactly,
            favorites_resume_exactly,
            audit_log_reads_newest_first_in_pages,
            refund_periods_round_trip,
            tickets_are_revoked_once,
            refunded_tickets_return_to_stock,
            refunds_are_recorded_once_and_completed_once,
            denials_are_idempotent_and_versioned,
            denial_listings_resume_exactly,
            denied_tickets_join_every_cause,
            organizations_are_versioned,
            public_organizations_have_a_public_event,
            account_listings_filter_and_members_resume_exactly,
            event_reservations_resume_exactly,
            event_refunds_resume_exactly,
            drafts_without_sales_are_deleted_with_what_hangs_on_them,
            lookups_by_slug_and_favorite,
            a_buyers_tickets_by_event_and_refunds_resume_exactly,
            profiles_change_until_the_account_closes,
            closed_accounts_keep_their_records,
            closed_accounts_retire_their_email,
            orders_count_while_someone_buys,
        );
    };
    (@cases $mode:tt; $($case:ident),* $(,)?) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                let Some(harness) = $crate::conformance_tests!(@connect $mode) else {
                    eprintln!("skipped: no database configured for this adapter");
                    return;
                };
                $crate::conformance::$case(harness.store.clone()).await;
            }
        )*
    };
    (@connect (required $connect:expr)) => {
        Some::<$crate::conformance::Harness>($connect.await)
    };
    (@connect (optional $connect:expr)) => {
        $connect.await as Option<$crate::conformance::Harness>
    };
}

// ── Fixtures ─────────────────────────────────────────────────────────────────────────────

fn now() -> Timestamp {
    Timestamp::from_unix_seconds(1_800_000_000)
}

fn unique_suffix() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

async fn account(store: &dyn Store) -> AccountId {
    let account = Account {
        id: AccountId::generate(),
        email: Some(Email::new(format!("{}@example.org", unique_suffix())).unwrap()),
        display_name: "Buyer".to_owned(),
        role: Role::User,
        created_at: now(),
        contacts: BTreeMap::new(),
        deleted_at: None,
    };
    store.insert_account(&account, Some("hash")).await.unwrap();
    account.id
}

async fn organization(store: &dyn Store) -> OrganizationId {
    let organization = Organization {
        id: OrganizationId::generate(),
        slug: Slug::new(format!("org-{}", unique_suffix())).unwrap(),
        name: "Organizer".to_owned(),
        created_at: now(),
        version: 1,
    };
    store.insert_organization(&organization).await.unwrap();
    organization.id
}

fn event_record(organization: OrganizationId) -> Event {
    Event {
        id: EventId::generate(),
        organization_id: organization,
        slug: Slug::new(format!("event-{}", unique_suffix())).unwrap(),
        title: "Convention".to_owned(),
        description: String::new(),
        venue: "Hall".to_owned(),
        address: None,
        starts_at: now(),
        ends_at: now() + Duration::days(2),
        status: EventStatus::Published,
        content: String::new(),
        created_at: now(),
        updated_at: now(),
        version: 1,
    }
}

/// A sale with one ticket type per capacity in `capacities`.
struct Catalog {
    event: EventId,
    sale: SaleId,
    ticket_types: Vec<TicketTypeId>,
}

async fn catalog(store: &dyn Store, capacities: &[u32]) -> Catalog {
    catalog_in(store, organization(store).await, capacities).await
}

/// Like [`catalog`], for an event of an existing organization.
async fn catalog_in(
    store: &dyn Store,
    organization: OrganizationId,
    capacities: &[u32],
) -> Catalog {
    let event = event_record(organization);
    store.insert_event(&event).await.unwrap();
    let sale = Sale {
        id: SaleId::generate(),
        event_id: event.id,
        name: "General".to_owned(),
        opens_at: now(),
        closes_at: now() + Duration::days(1),
        admission: AdmissionPolicy::Open,
        reservation_ttl_seconds: 600,
        max_tickets_per_request: 10,
        accepted_attestors: vec![AttestorId::MANUAL],
        environment: Environment::Live,
        created_at: now(),
        version: 1,
    };
    store.insert_sale(&sale).await.unwrap();
    let mut ticket_types = Vec::new();
    for &capacity in capacities {
        let ticket_type = TicketType {
            id: TicketTypeId::generate(),
            sale_id: sale.id,
            event_id: event.id,
            name: "Day pass".to_owned(),
            price: Money::new(1_000, Currency::JPY).unwrap(),
            capacity,
            per_account_limit: 4,
            valid_from: now(),
            valid_until: now() + Duration::days(2),
            ticket_extensions: BTreeMap::new(),
            refundable_until: None,
            created_at: now(),
            version: 1,
        };
        store.insert_ticket_type(&ticket_type).await.unwrap();
        ticket_types.push(ticket_type.id);
    }
    ticket_types.sort();
    Catalog {
        event: event.id,
        sale: sale.id,
        ticket_types,
    }
}

fn line(ticket_type: TicketTypeId, quantity: u32) -> LineItem {
    LineItem {
        ticket_type_id: ticket_type,
        quantity,
    }
}

fn lines<const N: usize>(lines: [LineItem; N]) -> LineItems {
    LineItems::new(lines)
}

fn purchase_request(account: AccountId, catalog: &Catalog, key: &str) -> PurchaseRequest {
    let key = IdempotencyKey::new(key).unwrap();
    PurchaseRequest {
        id: PurchaseRequestId::derive(account, catalog.sale, &key),
        account_id: account,
        sale_id: catalog.sale,
        basket: Basket::new(vec![line(catalog.ticket_types[0], 1)]).unwrap(),
        status: PurchaseStatus::Queued,
        created_at: now(),
        updated_at: now(),
    }
}

/// Records a purchase request and a reservation for it, returning the reservation.
async fn reservation(
    store: &dyn Store,
    catalog: &Catalog,
    status: ReservationStatus,
    expires_at: Timestamp,
) -> Reservation {
    let buyer = account(store).await;
    let request = purchase_request(buyer, catalog, &unique_suffix());
    store.insert_purchase_request(&request).await.unwrap();
    let price = Money::new(1_000, Currency::JPY).unwrap();
    let reservation = Reservation {
        id: ReservationId::generate(),
        purchase_request_id: request.id,
        account_id: buyer,
        sale_id: catalog.sale,
        event_id: catalog.event,
        items: vec![ReservedItem {
            ticket_type_id: catalog.ticket_types[0],
            quantity: 1,
            unit_price: price,
        }],
        total: price,
        environment: Environment::Live,
        status,
        attestor_id: None,
        expires_at,
        created_at: now(),
        updated_at: now(),
    };
    let mut tx = store.begin().await.unwrap();
    tx.reservations()
        .insert_reservation(&reservation)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    reservation
}

// ── Cases ────────────────────────────────────────────────────────────────────────────────

pub async fn migrations_are_idempotent(store: Arc<dyn Store>) {
    store.migrate().await.unwrap();
    store.migrate().await.unwrap();
    store.ping().await.unwrap();
}

pub async fn unique_keys_report_conflicts(store: Arc<dyn Store>) {
    let account = Account {
        id: AccountId::generate(),
        email: Some(Email::new(format!("{}@example.org", unique_suffix())).unwrap()),
        display_name: "Twin".to_owned(),
        role: Role::User,
        created_at: now(),
        contacts: BTreeMap::new(),
        deleted_at: None,
    };
    store.insert_account(&account, Some("hash")).await.unwrap();
    let twin = Account {
        id: AccountId::generate(),
        ..account
    };
    assert!(matches!(
        store.insert_account(&twin, Some("hash")).await,
        Err(StoreError::Conflict("email"))
    ));

    let organization = organization(store.as_ref()).await;
    let event = event_record(organization);
    store.insert_event(&event).await.unwrap();
    let same_slug = Event {
        id: EventId::generate(),
        ..event
    };
    assert!(matches!(
        store.insert_event(&same_slug).await,
        Err(StoreError::Conflict("slug"))
    ));
}

pub async fn stale_versions_conflict(store: Arc<dyn Store>) {
    let event = event_record(organization(store.as_ref()).await);
    store.insert_event(&event).await.unwrap();

    let renamed = Event {
        title: "Renamed".to_owned(),
        version: 2,
        ..event.clone()
    };
    store.update_event(&renamed, 1).await.unwrap();
    let stale = Event {
        title: "Stale".to_owned(),
        version: 2,
        ..event
    };
    assert!(matches!(
        store.update_event(&stale, 1).await,
        Err(StoreError::Conflict("version"))
    ));
    assert_eq!(
        store.event(renamed.id).await.unwrap().unwrap().title,
        "Renamed"
    );
}

pub async fn purchase_requests_are_idempotent(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let buyer = account(store.as_ref()).await;
    let request = purchase_request(buyer, &catalog, "retry-me");

    assert_eq!(
        store.insert_purchase_request(&request).await.unwrap(),
        Insertion::Inserted
    );
    for _ in 0..9 {
        assert_eq!(
            store.insert_purchase_request(&request).await.unwrap(),
            Insertion::Existing(request.clone())
        );
    }
    assert_eq!(
        store
            .queued_purchase_count(catalog.sale, 100)
            .await
            .unwrap(),
        1
    );
    for key in ["second", "third"] {
        store
            .insert_purchase_request(&purchase_request(buyer, &catalog, key))
            .await
            .unwrap();
    }
    assert_eq!(
        store
            .queued_purchase_count(catalog.sale, 100)
            .await
            .unwrap(),
        3
    );
    // Counting stops at the cap.
    assert_eq!(
        store.queued_purchase_count(catalog.sale, 2).await.unwrap(),
        2
    );
}

pub async fn inventory_never_oversells(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[100]).await;
    let ticket_type = catalog.ticket_types[0];

    let attempts = (0..300).map(|_| {
        let store = store.clone();
        tokio::spawn(async move {
            let mut tx = store.begin().await.unwrap();
            let held = tx
                .inventory()
                .try_hold(&lines([line(ticket_type, 1)]))
                .await
                .unwrap();
            tx.commit().await.unwrap();
            held
        })
    });
    let mut granted = 0;
    for attempt in attempts.collect::<Vec<_>>() {
        if attempt.await.unwrap() {
            granted += 1;
        }
    }

    assert_eq!(granted, 100);
    let inventory = store.inventory(ticket_type).await.unwrap().unwrap();
    assert_eq!(
        (inventory.held, inventory.sold, inventory.available()),
        (100, 0, 0)
    );
}

pub async fn sale_inventory_reads_every_type_at_once(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[5, 7, 9]).await;
    let other = catalog_in(store.as_ref(), organization(store.as_ref()).await, &[3]).await;
    let mut tx = store.begin().await.unwrap();
    assert!(
        tx.inventory()
            .try_hold(&lines([line(catalog.ticket_types[1], 2)]))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    let mut one_by_one = Vec::new();
    for &ticket_type in &catalog.ticket_types {
        one_by_one.push(store.inventory(ticket_type).await.unwrap().unwrap());
    }
    assert_eq!(
        store.sale_inventory(catalog.sale).await.unwrap(),
        one_by_one
    );
    assert_eq!(store.sale_inventory(other.sale).await.unwrap().len(), 1);
    assert_eq!(
        store.sale_inventory(SaleId::generate()).await.unwrap(),
        Vec::new()
    );
}

pub async fn stock_locked_up_front_never_deadlocks(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[1_000, 1_000]).await;
    let (first, second) = (catalog.ticket_types[0], catalog.ticket_types[1]);
    // Every transaction changes the two types in the order that deadlocks half the others,
    // after locking both in ticket type order.
    let transactions = (0..20).map(|index| {
        let store = store.clone();
        tokio::spawn(async move {
            let (a, b) = if index % 2 == 0 {
                (first, second)
            } else {
                (second, first)
            };
            let mut tx = store.begin().await?;
            tx.inventory()
                .lock_stock(&lines([line(second, 1), line(first, 1)]))
                .await?;
            for ticket_type in [a, b] {
                assert!(
                    tx.inventory()
                        .try_hold(&lines([line(ticket_type, 1)]))
                        .await?
                );
                tokio::task::yield_now().await;
            }
            tx.commit().await
        })
    });
    for transaction in transactions.collect::<Vec<_>>() {
        transaction.await.unwrap().unwrap();
    }
    assert_eq!(store.inventory(first).await.unwrap().unwrap().held, 20);
    assert_eq!(store.inventory(second).await.unwrap().unwrap().held, 20);
}

pub async fn holds_are_all_or_nothing(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[5, 1]).await;
    let (first, second) = (catalog.ticket_types[0], catalog.ticket_types[1]);

    let mut tx = store.begin().await.unwrap();
    let held = tx
        .inventory()
        .try_hold(&lines([line(first, 2), line(second, 2)]))
        .await
        .unwrap();
    assert!(!held, "the second ticket type has only one ticket");
    tx.commit().await.unwrap();
    assert_eq!(store.inventory(first).await.unwrap().unwrap().held, 0);

    let mut tx = store.begin().await.unwrap();
    assert!(
        !tx.inventory()
            .try_sell(&lines([line(first, 1), line(second, 2)]))
            .await
            .unwrap()
    );
    assert!(
        tx.inventory()
            .try_sell(&lines([line(first, 1), line(second, 1)]))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert_eq!(store.inventory(first).await.unwrap().unwrap().sold, 1);
    assert_eq!(
        store.inventory(second).await.unwrap().unwrap().available(),
        0
    );
}

pub async fn quotas_are_enforced_and_all_or_nothing(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[100, 100]).await;
    let (first, second) = (catalog.ticket_types[0], catalog.ticket_types[1]);
    let buyer = account(store.as_ref()).await;
    // Every type allows four tickets per account.
    let hold = |claims: &[(TicketTypeId, u32)]| {
        let items = LineItems::new(
            claims
                .iter()
                .map(|&(ticket_type, quantity)| line(ticket_type, quantity)),
        );
        Holds::new(&items, |_| Some(4)).unwrap()
    };

    let mut tx = store.begin().await.unwrap();
    assert!(
        tx.inventory()
            .try_take_quota(buyer, &hold(&[(first, 3)]))
            .await
            .unwrap()
    );
    // 3 + 2 > 4 on the first type: nothing may be taken on the second either.
    assert!(
        !tx.inventory()
            .try_take_quota(buyer, &hold(&[(first, 2), (second, 4)]))
            .await
            .unwrap()
    );
    assert!(
        tx.inventory()
            .try_take_quota(buyer, &hold(&[(second, 4)]))
            .await
            .unwrap()
    );
    tx.inventory()
        .return_quota(buyer, &lines([line(first, 3)]))
        .await
        .unwrap();
    assert!(
        tx.inventory()
            .try_take_quota(buyer, &hold(&[(first, 4)]))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
}

pub async fn dropped_transactions_roll_back(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let ticket_type = catalog.ticket_types[0];
    {
        let mut tx = store.begin().await.unwrap();
        assert!(
            tx.inventory()
                .try_hold(&lines([line(ticket_type, 3)]))
                .await
                .unwrap()
        );
        let event = IntegrationEvent::ReservationExpired {
            reservation_id: ReservationId::generate(),
        };
        tx.outbox().append_event(&event, now()).await.unwrap();
        // Dropped without commit.
    }
    assert_eq!(store.inventory(ticket_type).await.unwrap().unwrap().held, 0);
}

pub async fn a_purchase_queue_is_held_by_one_transaction(store: Arc<dyn Store>) {
    let other = catalog(store.as_ref(), &[10]).await;
    let catalog = catalog(store.as_ref(), &[10]).await;
    let mut first = store.begin().await.unwrap();
    assert!(
        first
            .purchases()
            .lock_purchase_queue(catalog.sale)
            .await
            .unwrap()
    );
    if !store.capabilities().concurrent_writers {
        // Write transactions never overlap: the queue is the transaction's anyway.
        first.commit().await.unwrap();
        return;
    }

    let mut second = store.begin().await.unwrap();
    let held = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        second.purchases().lock_purchase_queue(catalog.sale),
    )
    .await
    .expect("a held queue is refused at once, not waited for")
    .unwrap();
    assert!(!held, "one transaction at a time");
    assert!(
        second
            .purchases()
            .lock_purchase_queue(other.sale)
            .await
            .unwrap(),
        "other sales' queues are free"
    );

    first.commit().await.unwrap();
    let mut third = store.begin().await.unwrap();
    assert!(
        third
            .purchases()
            .lock_purchase_queue(catalog.sale)
            .await
            .unwrap(),
        "a commit lets the queue go"
    );
    drop(third);
    // A dropped transaction rolls back as its connection goes back to the pool.
    let mut taken = false;
    for _ in 0..50 {
        let mut fourth = store.begin().await.unwrap();
        if fourth
            .purchases()
            .lock_purchase_queue(catalog.sale)
            .await
            .unwrap()
        {
            taken = true;
            break;
        }
        drop(fourth);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(taken, "a dropped transaction lets the queue go");
    drop(second);
}

pub async fn queued_requests_are_read_in_arrival_order_with_their_denials(store: Arc<dyn Store>) {
    let organization = organization(store.as_ref()).await;
    let elsewhere = catalog(store.as_ref(), &[10]).await;
    let catalog = catalog_in(store.as_ref(), organization, &[10]).await;
    let mut buyers = Vec::new();
    for _ in 0..5 {
        buyers.push(account(store.as_ref()).await);
    }
    let at = |seconds| now() + Duration::seconds(seconds);
    let queue = |buyer, catalog: &Catalog, seconds| PurchaseRequest {
        created_at: at(seconds),
        updated_at: at(seconds),
        ..purchase_request(buyer, catalog, &unique_suffix())
    };
    let postponed = queue(buyers[4], &catalog, 0);
    let first = queue(buyers[0], &catalog, 1);
    let second = queue(buyers[1], &catalog, 2);
    let mut tied = [queue(buyers[2], &catalog, 3), queue(buyers[3], &catalog, 3)];
    tied.sort_by_key(|request| request.id);
    let other_sale = queue(buyers[0], &elsewhere, 0);
    for request in [&postponed, &first, &second, &tied[0], &tied[1], &other_sale] {
        store.insert_purchase_request(request).await.unwrap();
    }
    store
        .postpone_purchase_request(postponed.id, at(60))
        .await
        .unwrap();
    // One account denied for the event, another across the organization.
    for (event, buyer) in [
        (Some(catalog.event), first.account_id),
        (None, tied[1].account_id),
    ] {
        let denied = denial(organization, event, DenialSubject::Account(buyer));
        store.insert_denial(&denied).await.unwrap();
    }

    assert_eq!(
        store.sales_with_queued_requests(at(10), 100).await.unwrap(),
        vec![(elsewhere.sale, 1), (catalog.sale, 4)],
        "by their oldest due request"
    );
    assert_eq!(
        store.sales_with_queued_requests(at(10), 1).await.unwrap(),
        vec![(elsewhere.sale, 1)],
        "looking at no more requests than asked"
    );

    let read = async |now, limit| {
        let mut tx = store.begin().await.unwrap();
        assert!(
            tx.purchases()
                .lock_purchase_queue(catalog.sale)
                .await
                .unwrap()
        );
        tx.purchases()
            .queued_requests(catalog.sale, catalog.event, organization, now, limit)
            .await
            .unwrap()
            .into_iter()
            .map(|queued| (queued.request.id, queued.account_denied))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        read(at(10), 10).await,
        [
            (first.id, true),
            (second.id, false),
            (tied[0].id, false),
            (tied[1].id, true)
        ],
        "arrival order, ties by id, postponed requests and other sales left out"
    );
    assert_eq!(
        read(at(10), 2).await,
        [(first.id, true), (second.id, false)]
    );
    assert_eq!(
        read(at(60), 1).await,
        [(postponed.id, false)],
        "a postponed request is due again when its time comes"
    );

    let mut tx = store.begin().await.unwrap();
    tx.purchases()
        .set_purchase_status(
            first.id,
            PurchaseStatus::Rejected {
                reason: RejectionReason::AccountDenied,
            },
            at(10),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        read(at(10), 1).await,
        [(second.id, false)],
        "served requests are gone from the queue"
    );
}

pub async fn attestations_are_recorded_once(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let reservation = reservation(
        store.as_ref(),
        &catalog,
        ReservationStatus::PaymentPending,
        now(),
    )
    .await;
    let attestation = PaymentAttestation {
        attestor_id: AttestorId::MANUAL,
        attestation_id: format!("pay-{}", unique_suffix()),
        reservation_id: reservation.id,
        amount: reservation.total,
        occurred_at: now(),
        received_at: now(),
        disposition: PaymentDisposition::Applied,
    };

    let mut tx = store.begin().await.unwrap();
    assert_eq!(
        tx.payments()
            .insert_attestation(&attestation)
            .await
            .unwrap(),
        Insertion::Inserted
    );
    tx.commit().await.unwrap();

    for _ in 0..9 {
        let retry = PaymentAttestation {
            received_at: now() + Duration::seconds(5),
            ..attestation.clone()
        };
        let mut tx = store.begin().await.unwrap();
        assert_eq!(
            tx.payments().insert_attestation(&retry).await.unwrap(),
            Insertion::Existing(attestation.clone())
        );
        tx.commit().await.unwrap();
    }
    assert_eq!(
        store
            .attestations_for_reservation(reservation.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

pub async fn one_reservation_per_purchase_request(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let first = reservation(store.as_ref(), &catalog, ReservationStatus::Reserved, now()).await;
    let second = Reservation {
        id: ReservationId::generate(),
        ..first
    };
    let mut tx = store.begin().await.unwrap();
    let result = tx.reservations().insert_reservation(&second).await;
    assert!(matches!(
        result,
        Err(StoreError::Conflict("purchase_request_id"))
    ));
}

pub async fn waiting_room_positions_are_distinct(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let mut buyers = Vec::new();
    for _ in 0..100 {
        buyers.push(account(store.as_ref()).await);
    }
    let joins = buyers.into_iter().map(|buyer| {
        let store = store.clone();
        let sale = catalog.sale;
        tokio::spawn(async move { store.join_waiting_room(sale, buyer).await.unwrap() })
    });
    let mut positions = Vec::new();
    for join in joins.collect::<Vec<_>>() {
        positions.push(join.await.unwrap());
    }
    positions.sort_unstable();
    assert_eq!(positions, (1..=100).collect::<Vec<_>>());
    assert_eq!(
        store
            .waiting_room(catalog.sale)
            .await
            .unwrap()
            .last_position,
        100
    );
}

pub async fn an_account_keeps_its_waiting_room_position(store: Arc<dyn Store>) {
    let other_sale = catalog(store.as_ref(), &[10]).await;
    let catalog = catalog(store.as_ref(), &[10]).await;
    let (buyer, other) = (account(store.as_ref()).await, account(store.as_ref()).await);

    let joins = (0..8).map(|_| {
        let store = store.clone();
        let sale = catalog.sale;
        tokio::spawn(async move { store.join_waiting_room(sale, buyer).await.unwrap() })
    });
    let mut positions = HashSet::new();
    for join in joins.collect::<Vec<_>>() {
        positions.insert(join.await.unwrap());
    }
    assert_eq!(positions.len(), 1, "simultaneous joins share one position");
    let position = positions.into_iter().next().unwrap();
    assert_eq!(
        store.join_waiting_room(catalog.sale, buyer).await.unwrap(),
        position,
        "joining again returns the same position"
    );

    let others = store.join_waiting_room(catalog.sale, other).await.unwrap();
    assert_ne!(others, position, "another account gets another position");
    let room = store.waiting_room(catalog.sale).await.unwrap();
    assert!(others <= room.last_position && position <= room.last_position);
    assert_eq!(
        store
            .join_waiting_room(other_sale.sale, buyer)
            .await
            .unwrap(),
        1,
        "each sale has a queue of its own"
    );
}

pub async fn waiting_room_advances_once(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    for _ in 0..10 {
        let buyer = account(store.as_ref()).await;
        store.join_waiting_room(catalog.sale, buyer).await.unwrap();
    }
    assert!(
        store
            .advance_waiting_room(catalog.sale, 0, 5)
            .await
            .unwrap()
    );
    assert!(
        !store
            .advance_waiting_room(catalog.sale, 0, 5)
            .await
            .unwrap(),
        "stale compare-and-set"
    );
    assert!(
        !store
            .advance_waiting_room(catalog.sale, 5, 11)
            .await
            .unwrap(),
        "beyond last position"
    );
    assert_eq!(
        store
            .waiting_room(catalog.sale)
            .await
            .unwrap()
            .admitted_through,
        5
    );
    let pending = store.pending_waiting_rooms().await.unwrap();
    assert!(pending.contains(&catalog.sale), "people are still waiting");
    assert!(
        store
            .advance_waiting_room(catalog.sale, 5, 10)
            .await
            .unwrap()
    );
    let pending = store.pending_waiting_rooms().await.unwrap();
    assert!(!pending.contains(&catalog.sale), "everyone was admitted");
}

pub async fn outbox_is_transactional_and_ordered(store: Arc<dyn Store>) {
    let start = store
        .outbox_after(0, 10_000)
        .await
        .unwrap()
        .last()
        .map_or(0, |record| record.sequence);
    let expired = |_: usize| IntegrationEvent::ReservationExpired {
        reservation_id: ReservationId::generate(),
    };

    let committed: Vec<_> = (0..3).map(expired).collect();
    let mut tx = store.begin().await.unwrap();
    for event in &committed {
        tx.outbox().append_event(event, now()).await.unwrap();
    }
    tx.commit().await.unwrap();

    let mut tx = store.begin().await.unwrap();
    tx.outbox().append_event(&expired(0), now()).await.unwrap();
    drop(tx);

    let records = store.outbox_after(start, 100).await.unwrap();
    let events: Vec<_> = records.iter().map(|record| record.event.clone()).collect();
    assert_eq!(events, committed);
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
}

pub async fn sessions_rotate_once(store: Arc<dyn Store>) {
    let buyer = account(store.as_ref()).await;
    let session = |hash: &str| Session {
        id: SessionId::generate(),
        account_id: buyer,
        refresh_token_hash: hash.to_owned(),
        created_at: now(),
        expires_at: now() + Duration::days(30),
    };
    let first = format!("first-{}", unique_suffix());
    store.insert_session(&session(&first)).await.unwrap();

    let renewal = |hash: String| SessionRenewal {
        id: SessionId::generate(),
        refresh_token_hash: hash,
        created_at: now(),
        expires_at: now() + Duration::days(30),
    };
    let next = renewal(format!("next-{}", unique_suffix()));
    assert_eq!(
        store.rotate_session(&first, &next).await.unwrap(),
        Some(buyer)
    );
    let replay = renewal(format!("replay-{}", unique_suffix()));
    assert_eq!(store.rotate_session(&first, &replay).await.unwrap(), None);
    assert_eq!(
        store
            .rotate_session(&next.refresh_token_hash, &replay)
            .await
            .unwrap(),
        Some(buyer),
        "the rotated session carries on"
    );
}

pub async fn only_holding_reservations_are_overdue(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let past = now() - Duration::minutes(1);
    let overdue = reservation(
        store.as_ref(),
        &catalog,
        ReservationStatus::PaymentPending,
        past,
    )
    .await;
    let issued = reservation(store.as_ref(), &catalog, ReservationStatus::Issued, past).await;
    let future = reservation(
        store.as_ref(),
        &catalog,
        ReservationStatus::Reserved,
        now() + Duration::hours(1),
    )
    .await;

    let found = store.overdue_reservations(now(), 1_000).await.unwrap();
    assert!(found.contains(&overdue.id));
    assert!(!found.contains(&issued.id));
    assert!(!found.contains(&future.id));
}

pub async fn capacity_cannot_drop_below_stock_in_use(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let ticket_type = catalog.ticket_types[0];
    let mut tx = store.begin().await.unwrap();
    assert!(
        tx.inventory()
            .try_hold(&lines([line(ticket_type, 6)]))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    let mut record = store.ticket_type(ticket_type).await.unwrap().unwrap();
    record.capacity = 5;
    record.version = 2;
    assert!(matches!(
        store.update_ticket_type(&record, 1).await,
        Err(StoreError::Conflict("capacity"))
    ));
    record.capacity = 6;
    store.update_ticket_type(&record, 1).await.unwrap();
    assert_eq!(
        store
            .inventory(ticket_type)
            .await
            .unwrap()
            .unwrap()
            .available(),
        0
    );
}

fn passwordless(email: Option<Email>) -> Account {
    Account {
        id: AccountId::generate(),
        email,
        display_name: "Social".to_owned(),
        role: Role::User,
        created_at: now(),
        contacts: BTreeMap::new(),
        deleted_at: None,
    }
}

fn identity(provider: &str, account_id: AccountId) -> Identity {
    Identity {
        provider: ProviderName::new(provider).unwrap(),
        subject: Subject::new(format!("subject-{}", unique_suffix())).unwrap(),
        account_id,
        created_at: now(),
    }
}

pub async fn accounts_may_lack_email_and_password(store: Arc<dyn Store>) {
    for _ in 0..2 {
        let account = passwordless(None);
        store.insert_account(&account, None).await.unwrap();
        assert_eq!(
            store.account(account.id).await.unwrap(),
            Some(account.clone())
        );
        assert_eq!(store.password_hash(account.id).await.unwrap(), None);
    }

    let email = Email::new(format!("{}@example.org", unique_suffix())).unwrap();
    let account = passwordless(Some(email.clone()));
    store.insert_account(&account, None).await.unwrap();
    let (found, hash) = store.account_credentials(&email).await.unwrap().unwrap();
    assert_eq!((found.id, hash), (account.id, None));

    assert!(
        store
            .set_password_hash(account.id, "new-hash")
            .await
            .unwrap()
    );
    assert_eq!(
        store.password_hash(account.id).await.unwrap().as_deref(),
        Some("new-hash")
    );
    assert!(
        !store
            .set_password_hash(AccountId::generate(), "x")
            .await
            .unwrap()
    );

    let other = passwordless(None);
    store.insert_account(&other, None).await.unwrap();
    assert!(matches!(
        store.set_email(other.id, &email).await,
        Err(StoreError::Conflict("email"))
    ));
    let fresh = Email::new(format!("{}@example.org", unique_suffix())).unwrap();
    assert!(store.set_email(other.id, &fresh).await.unwrap());
    assert_eq!(
        store.account(other.id).await.unwrap().unwrap().email,
        Some(fresh)
    );
}

pub async fn first_external_sign_ins_create_one_account(store: Arc<dyn Store>) {
    let first = passwordless(None);
    let link = identity("apple", first.id);
    let attempts = (0..8).map(|_| {
        let store = store.clone();
        let account = Account {
            id: AccountId::generate(),
            ..first.clone()
        };
        let link = Identity {
            account_id: account.id,
            ..link.clone()
        };
        tokio::spawn(async move {
            let outcome = store
                .create_account_with_identity(&account, &link)
                .await
                .unwrap();
            (account.id, outcome)
        })
    });
    let mut winners = Vec::new();
    let mut linked_to = HashSet::new();
    for attempt in attempts.collect::<Vec<_>>() {
        let (id, outcome) = attempt.await.unwrap();
        match outcome {
            Insertion::Inserted => {
                winners.push(id);
                linked_to.insert(id);
            }
            Insertion::Existing(existing) => {
                linked_to.insert(existing);
                assert_eq!(
                    store.account(id).await.unwrap(),
                    None,
                    "a loser left an account"
                );
            }
        }
    }
    assert_eq!(winners.len(), 1);
    assert_eq!(linked_to.len(), 1, "everyone ends up on the same account");
    assert_eq!(
        store
            .identity_account(&link.provider, &link.subject)
            .await
            .unwrap(),
        Some(winners[0])
    );

    // An email conflict creates nothing.
    let email = Email::new(format!("{}@example.org", unique_suffix())).unwrap();
    store
        .insert_account(&passwordless(Some(email.clone())), None)
        .await
        .unwrap();
    let clash = passwordless(Some(email));
    let clash_link = identity("apple", clash.id);
    assert!(matches!(
        store
            .create_account_with_identity(&clash, &clash_link)
            .await,
        Err(StoreError::Conflict("email"))
    ));
    assert_eq!(store.account(clash.id).await.unwrap(), None);
    assert_eq!(
        store
            .identity_account(&clash_link.provider, &clash_link.subject)
            .await
            .unwrap(),
        None
    );
}

pub async fn identities_link_to_one_account(store: Arc<dyn Store>) {
    let owner = account(store.as_ref()).await;
    let other = account(store.as_ref()).await;
    let wechat = identity("wechat", owner);
    assert_eq!(
        store.insert_identity(&wechat).await.unwrap(),
        Insertion::Inserted
    );
    assert_eq!(
        store.insert_identity(&wechat).await.unwrap(),
        Insertion::Existing(owner)
    );
    let stolen = Identity {
        account_id: other,
        ..wechat.clone()
    };
    assert_eq!(
        store.insert_identity(&stolen).await.unwrap(),
        Insertion::Existing(owner)
    );
    // The same subject at another provider is another identity.
    let qq = Identity {
        provider: ProviderName::new("qq").unwrap(),
        ..stolen
    };
    assert_eq!(
        store.insert_identity(&qq).await.unwrap(),
        Insertion::Inserted
    );

    let listed = store.identities(owner).await.unwrap();
    assert_eq!(listed, vec![wechat.clone()]);
    assert!(store.close_account(owner, None, now()).await.unwrap());
    assert_eq!(
        store
            .identity_account(&wechat.provider, &wechat.subject)
            .await
            .unwrap(),
        Some(owner),
        "identities stay with a closed account, so they never sign up anew"
    );
}

pub async fn the_last_sign_in_method_is_kept(store: Arc<dyn Store>) {
    let social = passwordless(None);
    let apple = identity("apple", social.id);
    assert_eq!(
        store
            .create_account_with_identity(&social, &apple)
            .await
            .unwrap(),
        Insertion::Inserted
    );
    let wechat = identity("wechat", social.id);
    store.insert_identity(&wechat).await.unwrap();

    // Two concurrent unlinks: one may succeed, the other must keep the last identity.
    let unlink = |identity: Identity| {
        let store = store.clone();
        tokio::spawn(async move {
            store
                .unlink_identity(identity.account_id, &identity.provider, &identity.subject)
                .await
                .unwrap()
        })
    };
    let (first, second) = (unlink(apple.clone()), unlink(wechat.clone()));
    let mut outcomes = vec![first.await.unwrap(), second.await.unwrap()];
    outcomes.sort_by_key(|outcome| *outcome == Unlink::Unlinked);
    assert_eq!(outcomes, vec![Unlink::LastSignInMethod, Unlink::Unlinked]);
    assert_eq!(store.identities(social.id).await.unwrap().len(), 1);
    assert_eq!(
        store
            .unlink_identity(social.id, &apple.provider, &Subject::new("nobody").unwrap())
            .await
            .unwrap(),
        Unlink::NotLinked
    );

    // With a password, the last identity can go.
    store.set_password_hash(social.id, "hash").await.unwrap();
    let remaining = store.identities(social.id).await.unwrap().remove(0);
    assert_eq!(
        store
            .unlink_identity(social.id, &remaining.provider, &remaining.subject)
            .await
            .unwrap(),
        Unlink::Unlinked
    );
}

pub async fn outbox_readers_never_skip_late_commits(store: Arc<dyn Store>) {
    const WRITERS: usize = 16;
    let start = store
        .outbox_after(0, 10_000)
        .await
        .unwrap()
        .last()
        .map_or(0, |record| record.sequence);

    // Each writer appends, then dawdles before committing, so commits finish out of order.
    let writers: Vec<_> = (0..WRITERS)
        .map(|index| {
            let store = store.clone();
            tokio::spawn(async move {
                let event = IntegrationEvent::ReservationExpired {
                    reservation_id: ReservationId::generate(),
                };
                let mut tx = store.begin().await.unwrap();
                tx.outbox().append_event(&event, now()).await.unwrap();
                let pause = u64::try_from((index * 7) % 13).unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(pause)).await;
                tx.commit().await.unwrap();
                event
            })
        })
        .collect();

    // A reader that remembers the last sequence it saw, as consumers do.
    let reader = {
        let store = store.clone();
        tokio::spawn(async move {
            let mut last = start;
            let mut seen = Vec::new();
            while seen.len() < WRITERS {
                for record in store.outbox_after(last, 100).await.unwrap() {
                    last = record.sequence;
                    seen.push(record.event);
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            seen
        })
    };
    let mut written = Vec::new();
    for writer in writers {
        written.push(writer.await.unwrap());
    }
    let seen = tokio::time::timeout(std::time::Duration::from_secs(10), reader)
        .await
        .expect("the reader skipped an event")
        .unwrap();
    let key = |event: &IntegrationEvent| format!("{event:?}");
    let mut written: Vec<_> = written.iter().map(key).collect();
    let mut seen: Vec<_> = seen.iter().map(key).collect();
    written.sort();
    seen.sort();
    assert_eq!(seen, written);
}

fn webhook(organization: Option<OrganizationId>, delivered_through: i64) -> Webhook {
    Webhook {
        id: WebhookId::generate(),
        organization_id: organization,
        url: format!("https://hooks.example.org/{}", unique_suffix()),
        topics: Vec::new(),
        active: true,
        delivered_through,
        failures: 0,
        last_error: None,
        next_attempt_at: now(),
        created_at: now(),
        version: 1,
    }
}

async fn append_expired(store: &dyn Store) -> i64 {
    let mut tx = store.begin().await.unwrap();
    tx.outbox()
        .append_event(
            &IntegrationEvent::ReservationExpired {
                reservation_id: ReservationId::generate(),
            },
            now(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    store.latest_sequence().await.unwrap()
}

/// Claims every claimable webhook, returning their ids; leases end at `until`.
async fn claim_all(store: &dyn Store, at: Timestamp, until: Timestamp) -> HashSet<WebhookId> {
    let lease = Lease { now: at, until };
    let mut claimed = HashSet::new();
    while let Some(webhook) = store.claim_webhook(lease).await.unwrap() {
        assert!(claimed.insert(webhook.id), "claimed twice under one lease");
    }
    claimed
}

pub async fn webhooks_are_claimed_by_one_worker_when_due(store: Arc<dyn Store>) {
    let head = append_expired(store.as_ref()).await;
    assert!(head > 0);
    // Pending: behind the head. Caught up: nothing to deliver. Paused and not yet due: skipped.
    let pending = webhook(None, head - 1);
    let caught_up = webhook(None, head);
    let paused = Webhook {
        active: false,
        ..webhook(None, 0)
    };
    let later = Webhook {
        next_attempt_at: now() + Duration::minutes(5),
        ..webhook(None, 0)
    };
    for webhook in [&pending, &caught_up, &paused, &later] {
        store.insert_webhook(webhook).await.unwrap();
    }

    let until = now() + Duration::seconds(60);
    let claimed = claim_all(store.as_ref(), now(), until).await;
    assert!(claimed.contains(&pending.id));
    for skipped in [&caught_up, &paused, &later] {
        assert!(!claimed.contains(&skipped.id));
    }
    // Leased: nobody else gets it until the lease lapses.
    assert!(
        !claim_all(store.as_ref(), now(), until)
            .await
            .contains(&pending.id)
    );

    // A worker that outlived its lease cannot record over the next one's.
    let lapsed = now() + Duration::seconds(61);
    let next_until = lapsed + Duration::seconds(60);
    assert!(
        claim_all(store.as_ref(), lapsed, next_until)
            .await
            .contains(&pending.id)
    );
    let run = WebhookRun {
        delivered_through: head,
        failures: 0,
        last_error: None,
        next_attempt_at: lapsed,
    };
    assert!(
        !store
            .finish_webhook_run(pending.id, until, &run)
            .await
            .unwrap()
    );
    assert!(
        store
            .finish_webhook_run(pending.id, next_until, &run)
            .await
            .unwrap()
    );
    let finished = store.webhook(pending.id).await.unwrap().unwrap();
    assert_eq!(finished.delivered_through, head);
    // Caught up now, so not claimable even though its lease was released.
    assert!(
        !claim_all(
            store.as_ref(),
            next_until,
            next_until + Duration::seconds(60)
        )
        .await
        .contains(&pending.id)
    );
}

pub async fn webhook_settings_and_progress_are_separate(store: Arc<dyn Store>) {
    let organization = organization(store.as_ref()).await;
    let hook = webhook(Some(organization), 0);
    store.insert_webhook(&hook).await.unwrap();
    assert_eq!(store.webhook(hook.id).await.unwrap(), Some(hook.clone()));
    assert_eq!(
        store.list_webhooks(Some(organization)).await.unwrap(),
        vec![hook.clone()]
    );
    assert!(
        !store
            .list_webhooks(None)
            .await
            .unwrap()
            .iter()
            .any(|listed| listed.id == hook.id)
    );

    // Progress recorded by a run survives a settings update that read the webhook earlier.
    let until = now() + Duration::seconds(60);
    append_expired(store.as_ref()).await;
    let claimed = claim_all(store.as_ref(), now(), until).await;
    assert!(claimed.contains(&hook.id));
    let run = WebhookRun {
        delivered_through: 7,
        failures: 3,
        last_error: Some("HTTP 500".to_owned()),
        next_attempt_at: now() + Duration::seconds(8),
    };
    assert!(
        store
            .finish_webhook_run(hook.id, until, &run)
            .await
            .unwrap()
    );
    let edited = Webhook {
        url: "https://hooks.example.org/moved".to_owned(),
        topics: vec!["tickets.issued".to_owned()],
        active: false,
        version: 2,
        ..hook.clone()
    };
    store.update_webhook(&edited, 1).await.unwrap();
    let stored = store.webhook(hook.id).await.unwrap().unwrap();
    assert_eq!(
        (stored.url.as_str(), stored.active, stored.version),
        ("https://hooks.example.org/moved", false, 2)
    );
    assert_eq!(stored.topics, vec!["tickets.issued".to_owned()]);
    assert_eq!(
        (stored.delivered_through, stored.failures, stored.last_error),
        (7, 3, Some("HTTP 500".to_owned()))
    );
    assert!(matches!(
        store.update_webhook(&edited, 1).await,
        Err(StoreError::Conflict("version"))
    ));
    assert!(store.delete_webhook(hook.id).await.unwrap());
    assert!(!store.delete_webhook(hook.id).await.unwrap());
}

pub async fn event_images_keep_their_order(store: Arc<dyn Store>) {
    let event = event_record(organization(store.as_ref()).await);
    store.insert_event(&event).await.unwrap();
    let other = event_record(organization(store.as_ref()).await);
    store.insert_event(&other).await.unwrap();
    let image = |event_id: EventId, position: u32| EventImage {
        id: ImageId::generate(),
        event_id,
        format: ImageFormat::Webp,
        size_bytes: 1_234,
        position,
        created_at: now(),
    };
    let (first, second, third) = (image(event.id, 1), image(event.id, 2), image(event.id, 3));
    let elsewhere = image(other.id, 1);
    for image in [&first, &second, &third, &elsewhere] {
        store.insert_event_image(image).await.unwrap();
    }
    assert_eq!(
        store.event_image(second.id).await.unwrap(),
        Some(second.clone())
    );
    let ids =
        |images: Vec<EventImage>| images.into_iter().map(|image| image.id).collect::<Vec<_>>();
    assert_eq!(
        ids(store.event_images(event.id).await.unwrap()),
        vec![first.id, second.id, third.id]
    );

    store
        .set_image_positions(event.id, &[third.id, first.id, second.id, elsewhere.id])
        .await
        .unwrap();
    assert_eq!(
        ids(store.event_images(event.id).await.unwrap()),
        vec![third.id, first.id, second.id]
    );
    assert_eq!(
        ids(store.event_images(other.id).await.unwrap()),
        vec![elsewhere.id],
        "another event's image is not moved"
    );

    assert!(store.delete_event_image(first.id).await.unwrap());
    assert!(!store.delete_event_image(first.id).await.unwrap());
    assert_eq!(store.event_image(first.id).await.unwrap(), None);
}

/// Content is stored as is, and left out of listings and summaries.
pub async fn event_content_is_kept_whole_and_left_out_of_listings(store: Arc<dyn Store>) {
    let organization = organization(store.as_ref()).await;
    // The largest content allowed, with multi-byte characters: limits are in bytes.
    let page = "<p>駅弁</p>".repeat(MAX_EVENT_CONTENT_BYTES / "<p>駅弁</p>".len());
    let event = Event {
        content: page.clone(),
        ..event_record(organization)
    };
    store.insert_event(&event).await.unwrap();
    assert_eq!(store.event(event.id).await.unwrap().unwrap().content, page);

    let edited = Event {
        content: "{\"blocks\": []}".to_owned(),
        version: 2,
        ..event.clone()
    };
    store.update_event(&edited, 1).await.unwrap();
    assert_eq!(store.event(event.id).await.unwrap(), Some(edited.clone()));

    let filter = EventFilter {
        organization: Some(organization),
        ..EventFilter::default()
    };
    let listed = store
        .list_events(&filter, EventOrder::default(), PageRequest::first(10))
        .await
        .unwrap();
    assert_eq!(listed, vec![EventSummary::from(edited.clone())]);
    assert_eq!(
        store.event_summary(event.id).await.unwrap(),
        Some(EventSummary::from(edited))
    );
    assert_eq!(
        store.event_summary(EventId::generate()).await.unwrap(),
        None
    );
}

/// Reads a whole listing `size` records at a time, resuming after the last record of each
/// page the way a client follows cursors.
async fn walk<T, P: Copy>(
    size: u32,
    position: impl Fn(&T) -> P,
    mut read: impl AsyncFnMut(PageRequest<P>) -> Vec<T>,
) -> Vec<T> {
    let mut all = Vec::new();
    let mut page = PageRequest::first(size);
    loop {
        let records = read(page).await;
        assert!(
            records.len() <= size as usize,
            "a page holds at most `limit`"
        );
        let Some(last) = records.last() else {
            return all;
        };
        page.after = Some(position(last));
        all.extend(records);
    }
}

fn venue_address(country: &str, latitude: Option<f64>) -> Address {
    Address {
        country: CountryCode::new(country).unwrap(),
        region: Some("東京都".to_owned()),
        locality: None,
        postal_code: Some("135-0063".to_owned()),
        street: "有明3-11-1".to_owned(),
        latitude,
        longitude: latitude.map(|_| 139.794_236_7),
    }
}

pub async fn event_addresses_round_trip(store: Arc<dyn Store>) {
    let organization = organization(store.as_ref()).await;
    let event = Event {
        address: Some(venue_address("JP", Some(35.629_812_3))),
        ..event_record(organization)
    };
    store.insert_event(&event).await.unwrap();
    assert_eq!(store.event(event.id).await.unwrap(), Some(event.clone()));

    let moved_online = Event {
        venue: "Online".to_owned(),
        address: None,
        version: 2,
        ..event.clone()
    };
    store.update_event(&moved_online, 1).await.unwrap();
    assert_eq!(
        store.event(event.id).await.unwrap(),
        Some(moved_online.clone())
    );

    let back_on_site = Event {
        address: Some(venue_address("TW", None)),
        version: 3,
        ..moved_online
    };
    store.update_event(&back_on_site, 2).await.unwrap();
    assert_eq!(
        store.event(event.id).await.unwrap(),
        Some(back_on_site.clone())
    );
    let filter = EventFilter {
        organization: Some(organization),
        ..EventFilter::default()
    };
    let listed = store
        .list_events(&filter, EventOrder::default(), PageRequest::first(10))
        .await
        .unwrap();
    assert_eq!(listed, vec![EventSummary::from(back_on_site)]);
}

/// Four events of a new organization: the filter that lists them all, and the events.
async fn listed_events(store: &dyn Store) -> (EventFilter, [Event; 4]) {
    let organization = organization(store).await;
    let day = Duration::days(1);
    let second = Duration::seconds(1);
    let event =
        |starts_at: Timestamp, created_at: Timestamp, status, country: Option<&str>| Event {
            address: country.map(|country| venue_address(country, None)),
            starts_at,
            ends_at: starts_at + day,
            status,
            created_at,
            ..event_record(organization)
        };
    // Two pairs share a sort time, so resuming has to break ties by id.
    let events = [
        event(now() + day, now(), EventStatus::Published, Some("JP")),
        event(now() + day, now() + second, EventStatus::Draft, None),
        event(now() + day * 3, now(), EventStatus::Cancelled, Some("TW")),
        event(
            now() - day * 5,
            now() + second * 2,
            EventStatus::Published,
            Some("JP"),
        ),
    ];
    for event in &events {
        store.insert_event(event).await.unwrap();
    }
    let all = EventFilter {
        organization: Some(organization),
        ..EventFilter::default()
    };
    (all, events)
}

pub async fn event_listings_resume_exactly_in_every_order(store: Arc<dyn Store>) {
    let (all, events) = listed_events(store.as_ref()).await;
    let summaries: Vec<EventSummary> = events.iter().cloned().map(Into::into).collect();
    for order in [
        EventOrder::StartsAt,
        EventOrder::StartsAtDesc,
        EventOrder::CreatedAt,
        EventOrder::CreatedAtDesc,
    ] {
        let mut expected = summaries.clone();
        expected.sort_by_key(|event| {
            let position = order.position(event);
            (position.at, position.id)
        });
        if matches!(order, EventOrder::StartsAtDesc | EventOrder::CreatedAtDesc) {
            expected.reverse();
        }
        for size in [1, 3, 10] {
            let listed = walk(
                size,
                |event| order.position(event),
                async |page| store.list_events(&all, order, page).await.unwrap(),
            )
            .await;
            assert_eq!(listed, expected, "{order:?} in pages of {size}");
        }
    }
}

pub async fn event_listings_filter(store: Arc<dyn Store>) {
    let (all, events) = listed_events(store.as_ref()).await;
    let day = Duration::days(1);
    let ids = async |filter: EventFilter| {
        let listed = store
            .list_events(&filter, EventOrder::StartsAt, PageRequest::first(10))
            .await
            .unwrap();
        let mut ids: Vec<EventId> = listed.into_iter().map(|event| event.id).collect();
        ids.sort();
        ids
    };
    let expect = |picked: &[usize]| {
        let mut ids: Vec<EventId> = picked.iter().map(|&i| events[i].id).collect();
        ids.sort();
        ids
    };
    let japan = Some(CountryCode::new("JP").unwrap());
    let cases = [
        (
            EventFilter {
                country: japan,
                ..all.clone()
            },
            expect(&[0, 3]),
        ),
        (
            EventFilter {
                status: Some(EventStatus::Draft),
                ..all.clone()
            },
            expect(&[1]),
        ),
        (
            EventFilter {
                public_only: true,
                ..all.clone()
            },
            expect(&[0, 2, 3]),
        ),
        (
            EventFilter {
                starts_from: Some(now() + day),
                ..all.clone()
            },
            expect(&[0, 1, 2]),
        ),
        (
            EventFilter {
                starts_before: Some(now() + day),
                ..all.clone()
            },
            expect(&[3]),
        ),
        (
            EventFilter {
                ends_from: Some(now()),
                ..all.clone()
            },
            expect(&[0, 1, 2]),
        ),
        (
            EventFilter {
                ends_before: Some(now()),
                ..all.clone()
            },
            expect(&[3]),
        ),
        (
            EventFilter {
                starts_from: Some(now() + day),
                starts_before: Some(now() + day * 2),
                public_only: true,
                ..all.clone()
            },
            expect(&[0]),
        ),
    ];
    for (filter, expected) in cases {
        assert_eq!(ids(filter.clone()).await, expected, "{filter:?}");
    }
}

pub async fn account_listings_resume_exactly(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[100]).await;
    let buyer = account(store.as_ref()).await;
    let price = Money::new(1_000, Currency::JPY).unwrap();
    let mut reservations = Vec::new();
    let mut tickets = Vec::new();
    // Reservation times repeat once and ticket issue times twice, so ties need the id.
    for (i, offset) in [0, 0, 5, 9].into_iter().enumerate() {
        let at = now() + Duration::seconds(offset);
        let request = purchase_request(buyer, &catalog, &unique_suffix());
        store.insert_purchase_request(&request).await.unwrap();
        let reservation = Reservation {
            id: ReservationId::generate(),
            purchase_request_id: request.id,
            account_id: buyer,
            sale_id: catalog.sale,
            event_id: catalog.event,
            items: vec![ReservedItem {
                ticket_type_id: catalog.ticket_types[0],
                quantity: 2,
                unit_price: price,
            }],
            total: price,
            environment: Environment::Live,
            status: ReservationStatus::Issued,
            attestor_id: None,
            expires_at: at,
            created_at: at,
            updated_at: at,
        };
        let issued = (0..2).map(|_| Ticket {
            id: TicketId::generate(),
            reservation_id: reservation.id,
            account_id: buyer,
            event_id: catalog.event,
            ticket_type_id: catalog.ticket_types[0],
            valid_from: now(),
            valid_until: now() + Duration::days(2),
            issued_at: now() + Duration::seconds([0, 0, 0, 7][i]),
            status: TicketStatus::Valid,
            encoded: vec![1, 2, 3],
        });
        let issued: Vec<Ticket> = issued.collect();
        let mut tx = store.begin().await.unwrap();
        tx.reservations()
            .insert_reservation(&reservation)
            .await
            .unwrap();
        tx.tickets().insert_tickets(&issued).await.unwrap();
        tx.commit().await.unwrap();
        reservations.push(reservation);
        tickets.extend(issued);
    }

    // Newest first; equal times in id order.
    let newest_first = |at: Timestamp, id: uuid::Uuid| (std::cmp::Reverse(at), id);
    tickets.sort_by_key(|ticket| newest_first(ticket.issued_at, ticket.id.as_uuid()));
    reservations
        .sort_by_key(|reservation| newest_first(reservation.created_at, reservation.id.as_uuid()));
    for size in [1, 3, 100] {
        let listed = walk(
            size,
            |ticket: &Ticket| Keyset {
                at: ticket.issued_at,
                id: ticket.id.as_uuid(),
            },
            async |page| store.tickets_for_account(buyer, None, page).await.unwrap(),
        )
        .await;
        assert_eq!(listed, tickets, "tickets in pages of {size}");
        let listed = walk(
            size,
            |reservation: &Reservation| Keyset {
                at: reservation.created_at,
                id: reservation.id.as_uuid(),
            },
            async |page| store.reservations_for_account(buyer, page).await.unwrap(),
        )
        .await;
        assert_eq!(listed, reservations, "reservations in pages of {size}");
    }
}

pub async fn favorites_resume_exactly(store: Arc<dyn Store>) {
    let buyer = account(store.as_ref()).await;
    let organization = organization(store.as_ref()).await;
    let newest_first = |at: Timestamp, id: uuid::Uuid| (std::cmp::Reverse(at), id);
    let mut favorites = Vec::new();
    for offset in [3, 3, 1] {
        let event = event_record(organization);
        store.insert_event(&event).await.unwrap();
        let at = now() + Duration::seconds(offset);
        store.add_favorite(buyer, event.id, at).await.unwrap();
        favorites.push((event.id, at));
    }
    favorites.sort_by_key(|&(event, at)| newest_first(at, event.as_uuid()));
    for size in [1, 2, 10] {
        let listed = walk(
            size,
            |favorite: &crate::Favorite| favorite.position(),
            async |page| store.favorites(buyer, page).await.unwrap(),
        )
        .await;
        let listed: Vec<_> = listed
            .into_iter()
            .map(|favorite| (favorite.event_id, favorite.created_at))
            .collect();
        assert_eq!(listed, favorites, "favourites in pages of {size}");
    }
}

pub async fn audit_log_reads_newest_first_in_pages(store: Arc<dyn Store>) {
    let actor = format!("conformance:{}", unique_suffix());
    for action in ["first", "second", "third"] {
        let entry = AuditEntry {
            at: now(),
            actor: actor.clone(),
            action: action.to_owned(),
            target: "audit".to_owned(),
        };
        store.append_audit(&entry).await.unwrap();
    }
    // Only this case writes to the log, so the newest entries are the ones just appended.
    let newest = store.audit_log(PageRequest::first(2)).await.unwrap();
    let actions: Vec<&str> = newest
        .iter()
        .map(|record| record.entry.action.as_str())
        .collect();
    assert_eq!(actions, ["third", "second"]);
    assert!(newest.iter().all(|record| record.entry.actor == actor));
    assert!(newest[0].sequence > newest[1].sequence);

    let older = store
        .audit_log(PageRequest {
            limit: 1,
            after: Some(newest[1].sequence),
        })
        .await
        .unwrap();
    assert_eq!(older.len(), 1);
    assert_eq!(older[0].entry.action, "first");
    assert_eq!(older[0].entry.actor, actor);
}

// ── Refunds and denials ──────────────────────────────────────────────────────────────────

/// Issues `count` tickets of the catalog's first ticket type to a new buyer, the way a
/// payment does: the stock is held, then sold, and the tickets are recorded.
async fn issued(store: &dyn Store, catalog: &Catalog, count: u32) -> (Reservation, Vec<Ticket>) {
    let mut reservation = reservation(store, catalog, ReservationStatus::Issued, now()).await;
    reservation.items[0].quantity = count;
    let items = lines([line(catalog.ticket_types[0], count)]);
    let tickets: Vec<Ticket> = (0..count)
        .map(|_| Ticket {
            id: TicketId::generate(),
            reservation_id: reservation.id,
            account_id: reservation.account_id,
            event_id: catalog.event,
            ticket_type_id: catalog.ticket_types[0],
            valid_from: now(),
            valid_until: now() + Duration::days(2),
            issued_at: now(),
            status: TicketStatus::Valid,
            encoded: vec![1, 2, 3],
        })
        .collect();
    let mut tx = store.begin().await.unwrap();
    assert!(tx.inventory().try_hold(&items).await.unwrap());
    tx.inventory().sell_held(&items).await.unwrap();
    tx.tickets().insert_tickets(&tickets).await.unwrap();
    tx.commit().await.unwrap();
    (reservation, tickets)
}

pub async fn refund_periods_round_trip(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let mut ticket_type = store
        .ticket_type(catalog.ticket_types[0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ticket_type.refundable_until, None);

    ticket_type.refundable_until = Some(now() - Duration::hours(1));
    ticket_type.version = 2;
    store.update_ticket_type(&ticket_type, 1).await.unwrap();
    let stored = store.ticket_type(ticket_type.id).await.unwrap().unwrap();
    assert_eq!(stored.refundable_until, Some(now() - Duration::hours(1)));

    ticket_type.refundable_until = None;
    ticket_type.version = 3;
    store.update_ticket_type(&ticket_type, 2).await.unwrap();
    let stored = store.ticket_type(ticket_type.id).await.unwrap().unwrap();
    assert_eq!(stored.refundable_until, None);
}

pub async fn tickets_are_revoked_once(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let (_, tickets) = issued(store.as_ref(), &catalog, 2).await;
    let ids: Vec<TicketId> = tickets.iter().map(|ticket| ticket.id).collect();

    let racers = (0..8).map(|_| {
        let store = store.clone();
        let ids = ids.clone();
        tokio::spawn(async move {
            let mut tx = store.begin().await.unwrap();
            let revoked = tx.tickets().revoke_tickets(&ids).await.unwrap();
            tx.commit().await.unwrap();
            revoked
        })
    });
    let mut total = 0;
    for racer in racers {
        total += racer.await.unwrap();
    }
    assert_eq!(
        total, 2,
        "each ticket is revoked by exactly one transaction"
    );
    for id in &ids {
        let ticket = store.ticket(*id).await.unwrap().unwrap();
        assert_eq!(ticket.status, TicketStatus::Revoked);
    }

    // A dropped transaction revokes nothing.
    let (_, kept) = issued(store.as_ref(), &catalog, 1).await;
    let mut tx = store.begin().await.unwrap();
    assert_eq!(tx.tickets().revoke_tickets(&[kept[0].id]).await.unwrap(), 1);
    drop(tx);
    let ticket = store.ticket(kept[0].id).await.unwrap().unwrap();
    assert_eq!(ticket.status, TicketStatus::Valid);
}

pub async fn refunded_tickets_return_to_stock(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[3]).await;
    let ticket_type = catalog.ticket_types[0];
    issued(store.as_ref(), &catalog, 3).await;
    let inventory = store.inventory(ticket_type).await.unwrap().unwrap();
    assert_eq!((inventory.sold, inventory.available()), (3, 0));

    let mut tx = store.begin().await.unwrap();
    tx.inventory()
        .return_sold(&lines([line(ticket_type, 2)]))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let inventory = store.inventory(ticket_type).await.unwrap().unwrap();
    assert_eq!((inventory.sold, inventory.available()), (1, 2));

    // The returned stock can be held again; `sold` never goes below zero.
    let mut tx = store.begin().await.unwrap();
    assert!(
        tx.inventory()
            .try_hold(&lines([line(ticket_type, 2)]))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let mut tx = store.begin().await.unwrap();
    assert!(
        tx.inventory()
            .return_sold(&lines([line(ticket_type, 2)]))
            .await
            .is_err()
    );
    drop(tx);
    let inventory = store.inventory(ticket_type).await.unwrap().unwrap();
    assert_eq!((inventory.held, inventory.sold), (2, 1));
}

#[expect(clippy::too_many_lines, reason = "one refund's life, told in order")]
pub async fn refunds_are_recorded_once_and_completed_once(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let (reservation, tickets) = issued(store.as_ref(), &catalog, 3).await;
    let payment = PaymentAttestation {
        attestor_id: AttestorId::MANUAL,
        attestation_id: format!("pay-{}", unique_suffix()),
        reservation_id: reservation.id,
        amount: Money::new(3_000, Currency::JPY).unwrap(),
        occurred_at: now(),
        received_at: now(),
        disposition: PaymentDisposition::Applied,
    };
    let mut tx = store.begin().await.unwrap();
    tx.payments().insert_attestation(&payment).await.unwrap();
    tx.commit().await.unwrap();
    let first = Refund {
        id: RefundId::generate(),
        reservation_id: reservation.id,
        account_id: reservation.account_id,
        event_id: catalog.event,
        attestor_id: payment.attestor_id,
        attestation_id: payment.attestation_id.clone(),
        ticket_ids: vec![tickets[0].id, tickets[1].id],
        amount: Money::new(2_000, Currency::JPY).unwrap(),
        reason: RefundReason::Requested,
        status: RefundStatus::Pending,
        created_at: now(),
        completed_at: None,
    };
    let second = Refund {
        id: RefundId::reversal(AttestorId::MANUAL, &first.attestation_id),
        ticket_ids: vec![tickets[2].id],
        amount: Money::new(1_000, Currency::JPY).unwrap(),
        reason: RefundReason::Reversal,
        status: RefundStatus::Completed,
        created_at: now() + Duration::seconds(1),
        completed_at: Some(now() + Duration::seconds(1)),
        ..first.clone()
    };

    for refund in [&first, &second] {
        let mut tx = store.begin().await.unwrap();
        assert_eq!(
            tx.payments().insert_refund(refund).await.unwrap(),
            Insertion::Inserted
        );
        tx.commit().await.unwrap();
    }
    let retry = Refund {
        amount: Money::new(1, Currency::JPY).unwrap(),
        ..second.clone()
    };
    let mut tx = store.begin().await.unwrap();
    assert_eq!(
        tx.payments().insert_refund(&retry).await.unwrap(),
        Insertion::Existing(second.clone())
    );
    tx.commit().await.unwrap();

    assert_eq!(store.refund(first.id).await.unwrap(), Some(first.clone()));
    assert_eq!(
        store.refunds_for_reservation(reservation.id).await.unwrap(),
        vec![first.clone(), second.clone()]
    );
    assert_eq!(store.refund(RefundId::generate()).await.unwrap(), None);

    // Concurrent confirmations: one completes it, at the time it gives.
    let completed_at = now() + Duration::minutes(5);
    let racers = (0..6).map(|_| {
        let store = store.clone();
        let id = first.id;
        tokio::spawn(async move {
            let mut tx = store.begin().await.unwrap();
            let completed = tx
                .payments()
                .complete_refund(id, completed_at)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            completed
        })
    });
    let mut winners = 0;
    for racer in racers {
        winners += u32::from(racer.await.unwrap());
    }
    assert_eq!(winners, 1);
    let stored = store.refund(first.id).await.unwrap().unwrap();
    assert_eq!(stored.status, RefundStatus::Completed);
    assert_eq!(stored.completed_at, Some(completed_at));

    let mut tx = store.begin().await.unwrap();
    assert!(
        !tx.payments()
            .complete_refund(second.id, now())
            .await
            .unwrap()
    );
    assert!(
        !tx.payments()
            .complete_refund(RefundId::generate(), now())
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    assert_eq!(store.refund(second.id).await.unwrap(), Some(second));
}

fn denial(organization: OrganizationId, event: Option<EventId>, subject: DenialSubject) -> Denial {
    Denial {
        id: DenialId::derive(organization, event, subject),
        organization_id: organization,
        event_id: event,
        subject,
        note: "Resold on an auction site".to_owned(),
        created_at: now(),
        updated_at: now(),
        version: 1,
    }
}

pub async fn denials_are_idempotent_and_versioned(store: Arc<dyn Store>) {
    let organization = organization(store.as_ref()).await;
    let event = event_record(organization);
    store.insert_event(&event).await.unwrap();
    let denied = denial(
        organization,
        Some(event.id),
        DenialSubject::Account(AccountId::generate()),
    );

    assert_eq!(
        store.insert_denial(&denied).await.unwrap(),
        Insertion::Inserted
    );
    let again = Denial {
        note: "Different note".to_owned(),
        ..denied.clone()
    };
    assert_eq!(
        store.insert_denial(&again).await.unwrap(),
        Insertion::Existing(denied.clone())
    );
    assert_eq!(store.denial(denied.id).await.unwrap(), Some(denied.clone()));

    let edited = Denial {
        note: "Banned after an incident in 2026".to_owned(),
        updated_at: now() + Duration::hours(1),
        version: 2,
        ..denied.clone()
    };
    store.update_denial(&edited, 1).await.unwrap();
    let stale = Denial {
        note: "Stale".to_owned(),
        version: 2,
        ..denied.clone()
    };
    assert!(matches!(
        store.update_denial(&stale, 1).await,
        Err(StoreError::Conflict("version"))
    ));
    assert_eq!(store.denial(denied.id).await.unwrap(), Some(edited));

    // Organization-wide denials have no event.
    let everywhere = denial(
        organization,
        None,
        DenialSubject::Ticket(TicketId::generate()),
    );
    store.insert_denial(&everywhere).await.unwrap();
    assert_eq!(
        store.denial(everywhere.id).await.unwrap(),
        Some(everywhere.clone())
    );

    assert!(store.delete_denial(denied.id).await.unwrap());
    assert!(!store.delete_denial(denied.id).await.unwrap());
    assert_eq!(store.denial(denied.id).await.unwrap(), None);
    // Lifted, it can be added again.
    assert_eq!(
        store.insert_denial(&denied).await.unwrap(),
        Insertion::Inserted
    );
}

pub async fn denial_listings_resume_exactly(store: Arc<dyn Store>) {
    let organization = organization(store.as_ref()).await;
    let event = event_record(organization);
    store.insert_event(&event).await.unwrap();
    let other_event = event_record(organization);
    store.insert_event(&other_event).await.unwrap();

    let mut organization_wide = Vec::new();
    let mut for_event = Vec::new();
    for i in 0..5_i64 {
        // Pairs share a time, so ties are broken by id.
        let at = now() + Duration::seconds(i / 2);
        let wide = Denial {
            created_at: at,
            updated_at: at,
            ..denial(
                organization,
                None,
                DenialSubject::Account(AccountId::generate()),
            )
        };
        let own = Denial {
            created_at: at,
            updated_at: at,
            ..denial(
                organization,
                Some(event.id),
                DenialSubject::Ticket(TicketId::generate()),
            )
        };
        let elsewhere = denial(
            organization,
            Some(other_event.id),
            DenialSubject::Ticket(TicketId::generate()),
        );
        for denied in [&wide, &own, &elsewhere] {
            store.insert_denial(denied).await.unwrap();
        }
        organization_wide.push(wide);
        for_event.push(own);
    }

    let newest_first = |denial: &Denial| (std::cmp::Reverse(denial.created_at), denial.id);
    organization_wide.sort_by_key(newest_first);
    for_event.sort_by_key(newest_first);
    let position = |denial: &Denial| Keyset {
        at: denial.created_at,
        id: denial.id.as_uuid(),
    };
    for size in [1, 2, 100] {
        let listed = walk(size, position, async |page| {
            store
                .organization_denials(organization, page)
                .await
                .unwrap()
        })
        .await;
        assert_eq!(
            listed, organization_wide,
            "organization denials in pages of {size}"
        );
        let listed = walk(size, position, async |page| {
            store.event_denials(event.id, page).await.unwrap()
        })
        .await;
        assert_eq!(listed, for_event, "event denials in pages of {size}");
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "every cause of a denial set up side by side, then read back"
)]
pub async fn denied_tickets_join_every_cause(store: Arc<dyn Store>) {
    let here = catalog(store.as_ref(), &[100]).await;
    let organization = store
        .event(here.event)
        .await
        .unwrap()
        .unwrap()
        .organization_id;
    // A second event of the same organization, and an event of another organization.
    let sibling = catalog_in(store.as_ref(), organization, &[100]).await;
    let stranger = catalog(store.as_ref(), &[100]).await;
    let stranger_organization = store
        .event(stranger.event)
        .await
        .unwrap()
        .unwrap()
        .organization_id;

    let (_, refunded) = issued(store.as_ref(), &here, 2).await;
    let (_, ticket_denied) = issued(store.as_ref(), &here, 2).await;
    let (_, everywhere_denied) = issued(store.as_ref(), &here, 1).await;
    let (held_by_banned, banned_here) = issued(store.as_ref(), &here, 2).await;
    let (held_by_banned_everywhere, banned_everywhere) = issued(store.as_ref(), &here, 1).await;
    let (held_by_banned_elsewhere, _) = issued(store.as_ref(), &here, 1).await;
    let (_, untouched) = issued(store.as_ref(), &here, 2).await;
    let (_, sibling_tickets) = issued(store.as_ref(), &sibling, 1).await;

    let mut tx = store.begin().await.unwrap();
    tx.tickets()
        .revoke_tickets(&[refunded[0].id, refunded[1].id])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let denials = [
        // One ticket, for this event; and a revoked ticket, denied too.
        denial(
            organization,
            Some(here.event),
            DenialSubject::Ticket(ticket_denied[0].id),
        ),
        denial(
            organization,
            Some(here.event),
            DenialSubject::Ticket(refunded[0].id),
        ),
        // One ticket, for every event of the organization.
        denial(
            organization,
            None,
            DenialSubject::Ticket(everywhere_denied[0].id),
        ),
        // Accounts: for this event, for every event, and for the sibling event only.
        denial(
            organization,
            Some(here.event),
            DenialSubject::Account(held_by_banned.account_id),
        ),
        denial(
            organization,
            None,
            DenialSubject::Account(held_by_banned_everywhere.account_id),
        ),
        denial(
            organization,
            Some(sibling.event),
            DenialSubject::Account(held_by_banned_elsewhere.account_id),
        ),
        // Another organization's list does not reach this event.
        denial(
            stranger_organization,
            None,
            DenialSubject::Ticket(untouched[0].id),
        ),
        denial(
            stranger_organization,
            None,
            DenialSubject::Account(untouched[1].account_id),
        ),
    ];
    for denied in &denials {
        store.insert_denial(denied).await.unwrap();
    }

    let denied_ticket = |ticket: &Ticket, reason| DeniedTicket {
        ticket_id: ticket.id,
        ticket_type_id: ticket.ticket_type_id,
        reason,
    };
    let mut expected = vec![
        denied_ticket(&refunded[0], DeniedBecause::Revoked),
        denied_ticket(&refunded[1], DeniedBecause::Revoked),
        denied_ticket(&ticket_denied[0], DeniedBecause::Denied),
        denied_ticket(&everywhere_denied[0], DeniedBecause::Denied),
        denied_ticket(&banned_here[0], DeniedBecause::Denied),
        denied_ticket(&banned_here[1], DeniedBecause::Denied),
        denied_ticket(&banned_everywhere[0], DeniedBecause::Denied),
    ];
    expected.sort_by_key(|denied| denied.ticket_id);
    for size in [1, 3, 100] {
        let listed = walk(
            size,
            |denied: &DeniedTicket| denied.ticket_id.as_uuid(),
            async |page| store.denied_tickets(here.event, page).await.unwrap(),
        )
        .await;
        assert_eq!(listed, expected, "denied tickets in pages of {size}");
    }
    assert!(
        store
            .denied_tickets(sibling.event, PageRequest::first(100))
            .await
            .unwrap()
            .iter()
            .all(|denied| denied.ticket_id != sibling_tickets[0].id),
        "a ticket of the sibling event held by nobody denied is admitted"
    );
    assert_eq!(
        store
            .denied_tickets(stranger.event, PageRequest::first(100))
            .await
            .unwrap(),
        vec![],
    );

    for (account, expected) in [
        (held_by_banned.account_id, true),
        (held_by_banned_everywhere.account_id, true),
        (held_by_banned_elsewhere.account_id, false),
        (untouched[1].account_id, false),
    ] {
        assert_eq!(
            store
                .account_denied(organization, here.event, account)
                .await
                .unwrap(),
            expected
        );
    }
    assert!(
        store
            .account_denied(
                organization,
                sibling.event,
                held_by_banned_elsewhere.account_id
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .account_denied(
                stranger_organization,
                stranger.event,
                untouched[1].account_id
            )
            .await
            .unwrap()
    );

    // Lifting a denial lets its tickets in again.
    store.delete_denial(denials[3].id).await.unwrap();
    let listed = store
        .denied_tickets(here.event, PageRequest::first(100))
        .await
        .unwrap();
    assert!(
        listed
            .iter()
            .all(|denied| denied.ticket_id != banned_here[0].id)
    );
    assert_eq!(listed.len(), expected.len() - 2);
}

// ── Back-office reads ────────────────────────────────────────────────────────────────────

pub async fn organizations_are_versioned(store: Arc<dyn Store>) {
    let first = organization(store.as_ref()).await;
    let second = organization(store.as_ref()).await;
    let stored = store.organization(first).await.unwrap().unwrap();
    assert_eq!(stored.version, 1);

    let renamed = Organization {
        slug: Slug::new(format!("renamed-{}", unique_suffix())).unwrap(),
        name: "Renamed".to_owned(),
        version: 2,
        ..stored.clone()
    };
    store.update_organization(&renamed, 1).await.unwrap();
    assert_eq!(
        store.organization(first).await.unwrap(),
        Some(renamed.clone())
    );

    let stale = Organization {
        name: "Stale".to_owned(),
        version: 2,
        ..stored
    };
    assert!(matches!(
        store.update_organization(&stale, 1).await,
        Err(StoreError::Conflict("version"))
    ));
    let taken = Organization {
        slug: renamed.slug.clone(),
        version: 2,
        ..store.organization(second).await.unwrap().unwrap()
    };
    assert!(matches!(
        store.update_organization(&taken, 1).await,
        Err(StoreError::Conflict("slug"))
    ));
    let missing = Organization {
        id: OrganizationId::generate(),
        ..renamed.clone()
    };
    assert!(matches!(
        store.update_organization(&missing, 2).await,
        Err(StoreError::Conflict("version"))
    ));

    // Saving the same values is still an update of the version it names.
    let unchanged = Organization {
        version: 3,
        ..renamed.clone()
    };
    store.update_organization(&unchanged, 2).await.unwrap();
    assert_eq!(store.organization(first).await.unwrap(), Some(unchanged));
}

pub async fn public_organizations_have_a_public_event(store: Arc<dyn Store>) {
    use EventStatus::{Cancelled, Draft, Published};
    let mut public = Vec::new();
    let mut hidden = Vec::new();
    for statuses in [
        &[Published][..],
        &[Draft],
        &[Draft, Cancelled],
        &[],
        &[Published, Published, Draft],
    ] {
        let organization = organization(store.as_ref()).await;
        for &status in statuses {
            let event = Event {
                status,
                ..event_record(organization)
            };
            store.insert_event(&event).await.unwrap();
        }
        if statuses.iter().any(|&status| status != Draft) {
            public.push(organization);
        } else {
            hidden.push(organization);
        }
    }
    public.sort();

    for size in [1, 2, 10] {
        let listed = walk(
            size,
            |organization: &Organization| organization.id.as_uuid(),
            async |page| store.public_organizations(page).await.unwrap(),
        )
        .await;
        let ids: Vec<OrganizationId> = listed.iter().map(|organization| organization.id).collect();
        assert_eq!(ids, public, "public organizations in pages of {size}");
    }
    for &id in &public {
        let found = store.public_organization(id).await.unwrap();
        assert_eq!(found, store.organization(id).await.unwrap());
        assert!(found.is_some());
    }
    for &id in &hidden {
        assert_eq!(store.public_organization(id).await.unwrap(), None);
        assert!(store.organization(id).await.unwrap().is_some());
    }
    assert_eq!(
        store
            .public_organization(OrganizationId::generate())
            .await
            .unwrap(),
        None
    );
}

pub async fn account_listings_filter_and_members_resume_exactly(store: Arc<dyn Store>) {
    let mut accounts = Vec::new();
    for role in [
        Role::User,
        Role::Organizer,
        Role::Organizer,
        Role::Admin,
        Role::Organizer,
    ] {
        let account = Account {
            id: AccountId::generate(),
            email: Some(Email::new(format!("{}@example.org", unique_suffix())).unwrap()),
            display_name: "Someone".to_owned(),
            role,
            created_at: now(),
            contacts: BTreeMap::new(),
            deleted_at: None,
        };
        store.insert_account(&account, Some("hash")).await.unwrap();
        accounts.push(account);
    }
    let without_email = passwordless(None);
    store
        .create_account_with_identity(&without_email, &identity("apple", without_email.id))
        .await
        .unwrap();
    accounts.push(without_email);
    accounts.sort_by_key(|account| account.id);
    let organizers: Vec<Account> = accounts
        .iter()
        .filter(|account| account.role == Role::Organizer)
        .cloned()
        .collect();

    let list = |filter: AccountFilter| {
        let store = store.clone();
        async move |page| store.list_accounts(&filter, page).await.unwrap()
    };
    let by_id = |account: &Account| account.id.as_uuid();
    for size in [1, 2, 10] {
        let listed = walk(size, by_id, list(AccountFilter::default())).await;
        assert_eq!(listed, accounts, "every account in pages of {size}");
        let organizer_filter = AccountFilter {
            role: Some(Role::Organizer),
            email: None,
        };
        let listed = walk(size, by_id, list(organizer_filter)).await;
        assert_eq!(listed, organizers, "organizers in pages of {size}");
    }
    let wanted = organizers[1].clone();
    let by_email = AccountFilter {
        role: None,
        email: wanted.email.clone(),
    };
    assert_eq!(walk(10, by_id, list(by_email)).await, vec![wanted.clone()]);
    let wrong_role = AccountFilter {
        role: Some(Role::User),
        email: wanted.email.clone(),
    };
    assert_eq!(walk(10, by_id, list(wrong_role)).await, Vec::new());

    // Members in account id order; other organizations' members stay out.
    let elsewhere = organization(store.as_ref()).await;
    let organization = organization(store.as_ref()).await;
    for member in &organizers {
        store.add_member(organization, member.id).await.unwrap();
    }
    store.add_member(elsewhere, accounts[0].id).await.unwrap();
    for size in [1, 2, 10] {
        let listed = walk(size, by_id, async |page| {
            store.members(organization, page).await.unwrap()
        })
        .await;
        assert_eq!(listed, organizers, "members in pages of {size}");
    }
    let nobody = OrganizationId::generate();
    assert_eq!(
        store.members(nobody, PageRequest::first(10)).await.unwrap(),
        Vec::new()
    );
}

pub async fn event_reservations_resume_exactly(store: Arc<dyn Store>) {
    use ReservationStatus::{Cancelled, Issued, Reserved};
    let catalog = catalog(store.as_ref(), &[100]).await;
    let elsewhere = catalog_in(store.as_ref(), organization(store.as_ref()).await, &[100]).await;
    reservation(store.as_ref(), &elsewhere, Issued, now()).await;

    let mut reservations = Vec::new();
    // Creation times repeat, so ties need the id.
    for (offset, status) in [
        (0, Issued),
        (0, Reserved),
        (5, Issued),
        (9, Cancelled),
        (9, Issued),
    ] {
        let at = now() + Duration::seconds(offset);
        let buyer = account(store.as_ref()).await;
        let request = purchase_request(buyer, &catalog, &unique_suffix());
        store.insert_purchase_request(&request).await.unwrap();
        let price = Money::new(1_000, Currency::JPY).unwrap();
        let reservation = Reservation {
            id: ReservationId::generate(),
            purchase_request_id: request.id,
            account_id: buyer,
            sale_id: catalog.sale,
            event_id: catalog.event,
            items: vec![ReservedItem {
                ticket_type_id: catalog.ticket_types[0],
                quantity: 1,
                unit_price: price,
            }],
            total: price,
            environment: Environment::Live,
            status,
            attestor_id: None,
            expires_at: at + Duration::minutes(10),
            created_at: at,
            updated_at: at,
        };
        let mut tx = store.begin().await.unwrap();
        tx.reservations()
            .insert_reservation(&reservation)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        reservations.push(reservation);
    }
    // Newest first; equal times in id order.
    reservations.sort_by_key(|reservation| {
        (
            std::cmp::Reverse(reservation.created_at),
            reservation.id.as_uuid(),
        )
    });
    let issued: Vec<Reservation> = reservations
        .iter()
        .filter(|reservation| reservation.status == Issued)
        .cloned()
        .collect();

    let position = |reservation: &Reservation| Keyset {
        at: reservation.created_at,
        id: reservation.id.as_uuid(),
    };
    for size in [1, 2, 10] {
        for (status, expected) in [(None, &reservations), (Some(Issued), &issued)] {
            let listed = walk(size, position, async |page| {
                store
                    .reservations_for_event(catalog.event, status, page)
                    .await
                    .unwrap()
            })
            .await;
            assert_eq!(&listed, expected, "{status:?} in pages of {size}");
        }
    }
    let none = store
        .reservations_for_event(
            catalog.event,
            Some(ReservationStatus::Expired),
            PageRequest::first(10),
        )
        .await
        .unwrap();
    assert_eq!(none, Vec::new());
}

pub async fn event_refunds_resume_exactly(store: Arc<dyn Store>) {
    let elsewhere = catalog(store.as_ref(), &[100]).await;
    let catalog = catalog(store.as_ref(), &[100]).await;
    let mut refunds = Vec::new();
    let mut foreign = Vec::new();
    for (catalog, times) in [(&catalog, &[0, 0, 4, 7, 7][..]), (&elsewhere, &[0])] {
        let (reservation, tickets) = issued(store.as_ref(), catalog, 1).await;
        let payment = PaymentAttestation {
            attestor_id: AttestorId::MANUAL,
            attestation_id: format!("pay-{}", unique_suffix()),
            reservation_id: reservation.id,
            amount: Money::new(1_000, Currency::JPY).unwrap(),
            occurred_at: now(),
            received_at: now(),
            disposition: PaymentDisposition::Applied,
        };
        let mut tx = store.begin().await.unwrap();
        tx.payments().insert_attestation(&payment).await.unwrap();
        tx.commit().await.unwrap();
        for (i, &offset) in times.iter().enumerate() {
            let at = now() + Duration::seconds(offset);
            let completed = i % 2 == 1;
            let refund = Refund {
                id: RefundId::generate(),
                reservation_id: reservation.id,
                account_id: reservation.account_id,
                event_id: catalog.event,
                attestor_id: payment.attestor_id,
                attestation_id: payment.attestation_id.clone(),
                ticket_ids: vec![tickets[0].id],
                amount: Money::new(1_000, Currency::JPY).unwrap(),
                reason: RefundReason::Requested,
                status: if completed {
                    RefundStatus::Completed
                } else {
                    RefundStatus::Pending
                },
                created_at: at,
                completed_at: completed.then_some(at),
            };
            let mut tx = store.begin().await.unwrap();
            tx.payments().insert_refund(&refund).await.unwrap();
            tx.commit().await.unwrap();
            if catalog.event == elsewhere.event {
                foreign.push(refund);
            } else {
                refunds.push(refund);
            }
        }
    }
    // Oldest first; equal times in id order.
    refunds.sort_by_key(|refund| (refund.created_at, refund.id.as_uuid()));
    let pending: Vec<Refund> = refunds
        .iter()
        .filter(|refund| refund.status == RefundStatus::Pending)
        .cloned()
        .collect();

    let position = |refund: &Refund| Keyset {
        at: refund.created_at,
        id: refund.id.as_uuid(),
    };
    for size in [1, 2, 10] {
        for (status, expected) in [(None, &refunds), (Some(RefundStatus::Pending), &pending)] {
            let listed = walk(size, position, async |page| {
                store
                    .refunds_for_event(catalog.event, status, page)
                    .await
                    .unwrap()
            })
            .await;
            assert_eq!(&listed, expected, "{status:?} in pages of {size}");
        }
    }
    let listed = store
        .refunds_for_event(elsewhere.event, None, PageRequest::first(10))
        .await
        .unwrap();
    assert_eq!(listed, foreign);
}

pub async fn drafts_without_sales_are_deleted_with_what_hangs_on_them(store: Arc<dyn Store>) {
    let organization = organization(store.as_ref()).await;
    let buyer = account(store.as_ref()).await;
    let draft = Event {
        status: EventStatus::Draft,
        ..event_record(organization)
    };
    store.insert_event(&draft).await.unwrap();
    let image = EventImage {
        id: ImageId::generate(),
        event_id: draft.id,
        format: ImageFormat::Webp,
        size_bytes: 1_234,
        position: 1,
        created_at: now(),
    };
    store.insert_event_image(&image).await.unwrap();
    store.add_favorite(buyer, draft.id, now()).await.unwrap();
    let its_own = denial(organization, Some(draft.id), DenialSubject::Account(buyer));
    let everywhere = denial(organization, None, DenialSubject::Account(buyer));
    for denial in [&its_own, &everywhere] {
        store.insert_denial(denial).await.unwrap();
    }

    // Not a public event, nor a draft with a sale; then nothing is touched.
    let published = event_record(organization);
    store.insert_event(&published).await.unwrap();
    assert!(!store.delete_draft_event(published.id).await.unwrap());
    assert!(store.event(published.id).await.unwrap().is_some());
    let with_sale = catalog_in(store.as_ref(), organization, &[10]).await;
    let mut withdrawn = store.event(with_sale.event).await.unwrap().unwrap();
    withdrawn.status = EventStatus::Draft;
    withdrawn.version = 2;
    store.update_event(&withdrawn, 1).await.unwrap();
    let kept = denial(
        organization,
        Some(with_sale.event),
        DenialSubject::Account(buyer),
    );
    store.insert_denial(&kept).await.unwrap();
    assert!(!store.delete_draft_event(with_sale.event).await.unwrap());
    assert!(store.event(with_sale.event).await.unwrap().is_some());
    assert!(
        store.denial(kept.id).await.unwrap().is_some(),
        "a refused deletion deletes nothing"
    );
    assert!(!store.delete_draft_event(EventId::generate()).await.unwrap());

    assert!(store.delete_draft_event(draft.id).await.unwrap());
    assert!(store.event(draft.id).await.unwrap().is_none());
    assert_eq!(store.event_image(image.id).await.unwrap(), None);
    assert!(!store.is_favorite(buyer, draft.id).await.unwrap());
    assert_eq!(store.denial(its_own.id).await.unwrap(), None);
    assert!(
        store.denial(everywhere.id).await.unwrap().is_some(),
        "the organization's own denials stay"
    );
    assert!(!store.delete_draft_event(draft.id).await.unwrap());
}

pub async fn lookups_by_slug_and_favorite(store: Arc<dyn Store>) {
    let organization_id = organization(store.as_ref()).await;
    let organization = store.organization(organization_id).await.unwrap().unwrap();
    assert_eq!(
        store
            .organization_by_slug(&organization.slug)
            .await
            .unwrap(),
        Some(organization)
    );
    let event = event_record(organization_id);
    store.insert_event(&event).await.unwrap();
    let found = store.event_by_slug(&event.slug).await.unwrap().unwrap();
    assert_eq!(found.id, event.id);
    let nobody = Slug::new(format!("nobody-{}", unique_suffix())).unwrap();
    assert_eq!(store.organization_by_slug(&nobody).await.unwrap(), None);
    assert!(store.event_by_slug(&nobody).await.unwrap().is_none());

    let fan = account(store.as_ref()).await;
    let other = account(store.as_ref()).await;
    assert!(!store.is_favorite(fan, event.id).await.unwrap());
    store.add_favorite(fan, event.id, now()).await.unwrap();
    assert!(store.is_favorite(fan, event.id).await.unwrap());
    assert!(!store.is_favorite(other, event.id).await.unwrap());
    store.remove_favorite(fan, event.id).await.unwrap();
    assert!(!store.is_favorite(fan, event.id).await.unwrap());
}

/// Issues `count` tickets of the catalog's first type to `buyer` at `at`, paid through the
/// built-in manual attestor; returns the reservation, its payment and the tickets.
async fn issued_to(
    store: &dyn Store,
    catalog: &Catalog,
    buyer: AccountId,
    at: Timestamp,
    count: u32,
) -> (Reservation, PaymentAttestation, Vec<Ticket>) {
    let request = purchase_request(buyer, catalog, &unique_suffix());
    store.insert_purchase_request(&request).await.unwrap();
    let price = Money::new(1_000, Currency::JPY).unwrap();
    let reservation = Reservation {
        id: ReservationId::generate(),
        purchase_request_id: request.id,
        account_id: buyer,
        sale_id: catalog.sale,
        event_id: catalog.event,
        items: vec![ReservedItem {
            ticket_type_id: catalog.ticket_types[0],
            quantity: count,
            unit_price: price,
        }],
        total: price,
        environment: Environment::Live,
        status: ReservationStatus::Issued,
        attestor_id: None,
        expires_at: at,
        created_at: at,
        updated_at: at,
    };
    let tickets: Vec<Ticket> = (0..count)
        .map(|_| Ticket {
            id: TicketId::generate(),
            reservation_id: reservation.id,
            account_id: buyer,
            event_id: catalog.event,
            ticket_type_id: catalog.ticket_types[0],
            valid_from: now(),
            valid_until: now() + Duration::days(2),
            issued_at: at,
            status: TicketStatus::Valid,
            encoded: vec![1, 2, 3],
        })
        .collect();
    let payment = PaymentAttestation {
        attestor_id: AttestorId::MANUAL,
        attestation_id: format!("pay-{}", unique_suffix()),
        reservation_id: reservation.id,
        amount: price,
        occurred_at: at,
        received_at: at,
        disposition: PaymentDisposition::Applied,
    };
    let mut tx = store.begin().await.unwrap();
    tx.reservations()
        .insert_reservation(&reservation)
        .await
        .unwrap();
    tx.tickets().insert_tickets(&tickets).await.unwrap();
    tx.payments().insert_attestation(&payment).await.unwrap();
    tx.commit().await.unwrap();
    (reservation, payment, tickets)
}

pub async fn a_buyers_tickets_by_event_and_refunds_resume_exactly(store: Arc<dyn Store>) {
    let store = store.as_ref();
    let first = catalog(store, &[100]).await;
    let second = catalog(store, &[100]).await;
    let buyer = account(store).await;
    let someone_else = account(store).await;
    let later = now() + Duration::seconds(5);
    let mut bought = Vec::new();
    // Issue times repeat, so ties need the id.
    for (catalog, account, at, count) in [
        (&first, buyer, now(), 2),
        (&second, buyer, now(), 1),
        (&first, buyer, later, 1),
        (&first, someone_else, later, 1),
    ] {
        bought.push(issued_to(store, catalog, account, at, count).await);
    }

    let newest_first = |at: Timestamp, id: uuid::Uuid| (std::cmp::Reverse(at), id);
    let mut tickets: Vec<Ticket> = bought
        .iter()
        .flat_map(|(_, _, tickets)| tickets.iter().cloned())
        .filter(|ticket| ticket.account_id == buyer)
        .collect();
    tickets.sort_by_key(|ticket| newest_first(ticket.issued_at, ticket.id.as_uuid()));
    let at_first: Vec<Ticket> = tickets
        .iter()
        .filter(|ticket| ticket.event_id == first.event)
        .cloned()
        .collect();
    let position = |ticket: &Ticket| Keyset {
        at: ticket.issued_at,
        id: ticket.id.as_uuid(),
    };
    for size in [1, 2, 10] {
        for (event, expected) in [(None, &tickets), (Some(first.event), &at_first)] {
            let listed = walk(size, position, async |page| {
                store.tickets_for_account(buyer, event, page).await.unwrap()
            })
            .await;
            assert_eq!(&listed, expected, "{event:?} in pages of {size}");
        }
    }

    // Refunds, newest first, of this buyer only.
    let mut refunds = Vec::new();
    for (i, (reservation, payment, tickets)) in bought.iter().enumerate() {
        let refund = Refund {
            id: RefundId::generate(),
            reservation_id: reservation.id,
            account_id: reservation.account_id,
            event_id: reservation.event_id,
            attestor_id: payment.attestor_id,
            attestation_id: payment.attestation_id.clone(),
            ticket_ids: vec![tickets[0].id],
            amount: Money::new(1_000, Currency::JPY).unwrap(),
            reason: RefundReason::Requested,
            status: RefundStatus::Pending,
            created_at: now() + Duration::seconds([0, 3, 3, 0][i]),
            completed_at: None,
        };
        let mut tx = store.begin().await.unwrap();
        tx.payments().insert_refund(&refund).await.unwrap();
        tx.commit().await.unwrap();
        refunds.push(refund);
    }
    refunds.retain(|refund| refund.account_id == buyer);
    refunds.sort_by_key(|refund| newest_first(refund.created_at, refund.id.as_uuid()));
    for size in [1, 2, 10] {
        let listed = walk(
            size,
            |refund: &Refund| Keyset {
                at: refund.created_at,
                id: refund.id.as_uuid(),
            },
            async |page| store.refunds_for_account(buyer, page).await.unwrap(),
        )
        .await;
        assert_eq!(listed, refunds, "refunds in pages of {size}");
    }
}

// ── Profiles and closed accounts ─────────────────────────────────────────────────────────

pub async fn profiles_change_until_the_account_closes(store: Arc<dyn Store>) {
    let id = account(store.as_ref()).await;
    let contacts = BTreeMap::from([
        ("LINE".to_owned(), "miku39".to_owned()),
        ("X".to_owned(), "@miku".to_owned()),
    ]);
    assert!(store.update_profile(id, "Miku", &contacts).await.unwrap());
    let stored = store.account(id).await.unwrap().unwrap();
    assert_eq!(
        (stored.display_name.as_str(), &stored.contacts),
        ("Miku", &contacts)
    );
    let listed = store
        .list_accounts(&AccountFilter::default(), PageRequest::first(10))
        .await
        .unwrap();
    assert_eq!(listed, vec![stored]);

    assert!(store.close_account(id, None, now()).await.unwrap());
    assert!(!store.update_profile(id, "Back", &contacts).await.unwrap());
    assert!(!store.set_password_hash(id, "hash").await.unwrap());
    let email = Email::new(format!("{}@example.org", unique_suffix())).unwrap();
    assert!(!store.set_email(id, &email).await.unwrap());
    assert!(
        !store
            .update_profile(AccountId::generate(), "Nobody", &contacts)
            .await
            .unwrap()
    );
}

pub async fn closed_accounts_keep_their_records(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let (reservation, _, tickets) = {
        let buyer = account(store.as_ref()).await;
        issued_to(store.as_ref(), &catalog, buyer, now(), 1).await
    };
    let id = reservation.account_id;
    let before = store.account(id).await.unwrap().unwrap();
    let email = before.email.clone().unwrap();
    let organization = organization(store.as_ref()).await;
    store.add_member(organization, id).await.unwrap();
    store.add_favorite(id, catalog.event, now()).await.unwrap();
    let session = Session {
        id: SessionId::generate(),
        account_id: id,
        refresh_token_hash: format!("hash-{}", unique_suffix()),
        created_at: now(),
        expires_at: now() + Duration::days(1),
    };
    store.insert_session(&session).await.unwrap();
    let denied = denial(organization, None, DenialSubject::Account(id));
    store.insert_denial(&denied).await.unwrap();
    store
        .update_profile(
            id,
            "Miku",
            &BTreeMap::from([("X".to_owned(), "@miku".to_owned())]),
        )
        .await
        .unwrap();

    let closed_at = now() + Duration::hours(1);
    assert!(
        store
            .close_account(id, Some(&email.fingerprint()), closed_at)
            .await
            .unwrap()
    );
    assert!(
        !store.close_account(id, None, closed_at).await.unwrap(),
        "once"
    );
    assert!(
        !store
            .close_account(AccountId::generate(), None, closed_at)
            .await
            .unwrap()
    );

    // Nothing about the person remains, but everything recorded under the id does.
    let after = store.account(id).await.unwrap().unwrap();
    assert_eq!(
        after,
        Account {
            email: None,
            display_name: String::new(),
            contacts: BTreeMap::new(),
            deleted_at: Some(closed_at),
            ..before
        }
    );
    assert_eq!(store.password_hash(id).await.unwrap(), None);
    assert_eq!(store.account_credentials(&email).await.unwrap(), None);
    assert_eq!(store.memberships(id).await.unwrap(), Vec::new());
    assert!(!store.is_favorite(id, catalog.event).await.unwrap());
    let rotated = store
        .rotate_session(
            &session.refresh_token_hash,
            &SessionRenewal {
                id: SessionId::generate(),
                refresh_token_hash: format!("next-{}", unique_suffix()),
                created_at: now(),
                expires_at: now() + Duration::days(1),
            },
        )
        .await
        .unwrap();
    assert_eq!(rotated, None, "its sessions end");
    assert_eq!(
        store.reservation(reservation.id).await.unwrap(),
        Some(reservation)
    );
    assert_eq!(
        store.ticket(tickets[0].id).await.unwrap(),
        Some(tickets[0].clone())
    );
    assert!(store.denial(denied.id).await.unwrap().is_some());
}

pub async fn closed_accounts_retire_their_email(store: Arc<dyn Store>) {
    let id = account(store.as_ref()).await;
    let email = store.account(id).await.unwrap().unwrap().email.unwrap();
    assert!(
        store
            .close_account(id, Some(&email.fingerprint()), now())
            .await
            .unwrap()
    );
    let closed_at = now();

    // No account may take it, by signing up or by changing email.
    let newcomer = Account {
        id: AccountId::generate(),
        email: Some(email.clone()),
        display_name: "Newcomer".to_owned(),
        role: Role::User,
        created_at: now(),
        contacts: BTreeMap::new(),
        deleted_at: None,
    };
    assert!(matches!(
        store.insert_account(&newcomer, Some("hash")).await,
        Err(StoreError::Conflict("email"))
    ));
    let other = account(store.as_ref()).await;
    assert!(matches!(
        store.set_email(other, &email).await,
        Err(StoreError::Conflict("email"))
    ));
    // A fingerprint given at closing is kept for accounts whose store had none.
    let legacy = passwordless(Some(
        Email::new(format!("{}@example.org", unique_suffix())).unwrap(),
    ));
    store
        .create_account_with_identity(&legacy, &identity("apple", legacy.id))
        .await
        .unwrap();
    let legacy_email = legacy.email.clone().unwrap();
    assert!(
        store
            .close_account(legacy.id, Some(&legacy_email.fingerprint()), closed_at)
            .await
            .unwrap()
    );
    let reused = Account {
        id: AccountId::generate(),
        email: Some(legacy_email),
        ..newcomer
    };
    assert!(matches!(
        store.insert_account(&reused, None).await,
        Err(StoreError::Conflict("email"))
    ));
}

pub async fn orders_count_while_someone_buys(store: Arc<dyn Store>) {
    use ReservationStatus::{Cancelled, Expired, Issued, PaymentPending};
    let catalog = catalog(store.as_ref(), &[100]).await;
    assert!(!store.has_orders(catalog.event, None).await.unwrap());
    let given_up = [
        reservation(store.as_ref(), &catalog, Expired, now()).await,
        reservation(store.as_ref(), &catalog, Cancelled, now()).await,
    ];
    assert!(
        !store.has_orders(catalog.event, None).await.unwrap(),
        "expired and cancelled reservations are no orders"
    );
    assert!(
        !store
            .has_orders(catalog.event, Some(given_up[0].account_id))
            .await
            .unwrap()
    );

    let paying = reservation(store.as_ref(), &catalog, PaymentPending, now()).await;
    assert!(store.has_orders(catalog.event, None).await.unwrap());
    assert!(
        store
            .has_orders(catalog.event, Some(paying.account_id))
            .await
            .unwrap()
    );
    assert!(
        !store
            .has_orders(catalog.event, Some(given_up[1].account_id))
            .await
            .unwrap()
    );
    let elsewhere = catalog_in(store.as_ref(), organization(store.as_ref()).await, &[10]).await;
    reservation(store.as_ref(), &elsewhere, Issued, now()).await;
    let other_event = catalog_in(store.as_ref(), organization(store.as_ref()).await, &[10]).await;
    assert!(!store.has_orders(other_event.event, None).await.unwrap());
}
