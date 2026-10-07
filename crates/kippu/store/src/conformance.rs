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
    Event, EventStatus, EventSummary, MAX_EVENT_CONTENT_BYTES, Sale, TicketType,
};
use kippu_domain::image::{EventImage, ImageFormat};
use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::payment::{Environment, PaymentAttestation, PaymentDisposition};
use kippu_domain::purchase::{Basket, LineItem, PurchaseRequest, PurchaseStatus};
use kippu_domain::reservation::{Reservation, ReservationStatus, ReservedItem};
use kippu_domain::validation::{Email, IdempotencyKey, ProviderName, Slug, Subject};
use kippu_domain::webhook::Webhook;
use kippu_domain::{
    AccountId, AttestorId, Currency, Duration, EventId, ImageId, Money, OrganizationId,
    PurchaseRequestId, ReservationId, SaleId, SessionId, TicketTypeId, Timestamp, WebhookId,
};

use crate::{
    EventFilter, Hold, Insertion, Lease, PageRequest, Session, SessionRenewal, Store, StoreError,
    Unlink, WebhookRun,
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
            quotas_are_enforced_and_all_or_nothing,
            dropped_transactions_roll_back,
            claims_are_exclusive_until_the_lease_lapses,
            attestations_are_recorded_once,
            one_reservation_per_purchase_request,
            waiting_room_positions_are_distinct,
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
    let event = event_record(organization(store).await);
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
    assert_eq!(store.queued_purchase_count(catalog.sale).await.unwrap(), 1);
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
                .try_hold(&[line(ticket_type, 1)])
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

pub async fn holds_are_all_or_nothing(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[5, 1]).await;
    let (first, second) = (catalog.ticket_types[0], catalog.ticket_types[1]);

    let mut tx = store.begin().await.unwrap();
    let held = tx
        .inventory()
        .try_hold(&[line(first, 2), line(second, 2)])
        .await
        .unwrap();
    assert!(!held, "the second ticket type has only one ticket");
    tx.commit().await.unwrap();
    assert_eq!(store.inventory(first).await.unwrap().unwrap().held, 0);

    let mut tx = store.begin().await.unwrap();
    assert!(
        !tx.inventory()
            .try_sell(&[line(first, 1), line(second, 2)])
            .await
            .unwrap()
    );
    assert!(
        tx.inventory()
            .try_sell(&[line(first, 1), line(second, 1)])
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
    let hold = |ticket_type, quantity| Hold {
        ticket_type_id: ticket_type,
        quantity,
        per_account_limit: 4,
    };

    let mut tx = store.begin().await.unwrap();
    assert!(
        tx.inventory()
            .try_take_quota(buyer, &[hold(first, 3)])
            .await
            .unwrap()
    );
    // 3 + 2 > 4 on the first type: nothing may be taken on the second either.
    assert!(
        !tx.inventory()
            .try_take_quota(buyer, &[hold(first, 2), hold(second, 4)])
            .await
            .unwrap()
    );
    assert!(
        tx.inventory()
            .try_take_quota(buyer, &[hold(second, 4)])
            .await
            .unwrap()
    );
    tx.inventory()
        .return_quota(buyer, &[line(first, 3)])
        .await
        .unwrap();
    assert!(
        tx.inventory()
            .try_take_quota(buyer, &[hold(first, 4)])
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
                .try_hold(&[line(ticket_type, 3)])
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

pub async fn claims_are_exclusive_until_the_lease_lapses(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    let buyer = account(store.as_ref()).await;
    let mut expected = HashSet::new();
    for index in 0..40 {
        let request = purchase_request(buyer, &catalog, &format!("claim-{index}"));
        store.insert_purchase_request(&request).await.unwrap();
        expected.insert(request.id);
    }
    let lease = Lease {
        now: now(),
        until: now() + Duration::seconds(30),
    };

    let claimers = (0..8).map(|_| {
        let store = store.clone();
        tokio::spawn(async move { store.claim_purchase_requests(lease, 10).await.unwrap() })
    });
    let mut claimed = Vec::new();
    for claimer in claimers.collect::<Vec<_>>() {
        claimed.extend(claimer.await.unwrap().into_iter().map(|request| request.id));
    }
    let distinct: HashSet<_> = claimed.iter().copied().collect();
    assert_eq!(distinct.len(), claimed.len(), "a request was claimed twice");
    assert!(expected.is_subset(&distinct), "every request was claimed");

    let early = Lease {
        now: now() + Duration::seconds(29),
        until: now() + Duration::seconds(59),
    };
    let reclaimed = store.claim_purchase_requests(early, 100).await.unwrap();
    assert!(
        reclaimed
            .iter()
            .all(|request| !expected.contains(&request.id))
    );

    let late = Lease {
        now: now() + Duration::seconds(30),
        until: now() + Duration::seconds(60),
    };
    let reclaimed: HashSet<_> = store
        .claim_purchase_requests(late, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|request| request.id)
        .collect();
    assert!(
        expected.is_subset(&reclaimed),
        "lapsed leases can be claimed again"
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
    let joins = (0..100).map(|_| {
        let store = store.clone();
        let sale = catalog.sale;
        tokio::spawn(async move { store.join_waiting_room(sale).await.unwrap() })
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

pub async fn waiting_room_advances_once(store: Arc<dyn Store>) {
    let catalog = catalog(store.as_ref(), &[10]).await;
    for _ in 0..10 {
        store.join_waiting_room(catalog.sale).await.unwrap();
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
            .try_hold(&[line(ticket_type, 6)])
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
    assert!(store.delete_account(owner).await.unwrap());
    assert_eq!(
        store
            .identity_account(&wechat.provider, &wechat.subject)
            .await
            .unwrap(),
        None,
        "identities go with their account"
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
        public_only: false,
    };
    let listed = store
        .list_events(filter, PageRequest::first(10))
        .await
        .unwrap();
    assert_eq!(listed, vec![EventSummary::from(edited)]);
}
