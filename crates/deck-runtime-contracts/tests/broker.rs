use latentdeck_deck_runtime_contracts::{
    BrokerError, ContractId, MAX_WARM_SESSIONS, OutputPinKind, PackageIdentity, SessionBroker,
    SessionId, WarmSession, WorkerId,
};
use semver::Version;

fn package(package_id: &str) -> PackageIdentity {
    PackageIdentity::new(
        ContractId::new(package_id).unwrap(),
        Version::parse("1.0.0").unwrap(),
    )
}

fn session(number: usize) -> WarmSession {
    WarmSession {
        session_id: SessionId::new(format!("session-{number}")).unwrap(),
        worker_id: WorkerId::new(format!("worker-{number}")).unwrap(),
        deck: package("org.latentdeck.deck"),
        codec: package("org.latentdeck.codec"),
    }
}

fn session_id(number: usize) -> SessionId {
    SessionId::new(format!("session-{number}")).unwrap()
}

fn worker_id(number: usize) -> WorkerId {
    WorkerId::new(format!("worker-{number}")).unwrap()
}

fn broker_with_sessions(count: usize) -> SessionBroker {
    let mut broker = SessionBroker::new();
    for number in 1..=count {
        broker.open_session(session(number)).unwrap();
    }
    broker
}

#[test]
fn four_sessions_are_warm_and_the_fifth_is_rejected_without_eviction() {
    let mut broker = broker_with_sessions(MAX_WARM_SESSIONS);
    let before = broker.snapshot();

    let error = broker.open_session(session(5)).unwrap_err();

    assert_eq!(error, BrokerError::SessionCapacityExceeded);
    assert_eq!(error.code(), "session.capacity_exceeded");
    assert_eq!(error.to_string(), "session.capacity_exceeded");
    assert_eq!(
        serde_json::to_string(&error).unwrap(),
        "\"session.capacity_exceeded\""
    );
    assert_eq!(broker.snapshot(), before);
    assert_eq!(broker.len(), 4);
    for number in 1..=4 {
        assert!(broker.contains_session(&session_id(number)));
    }
    assert!(!broker.contains_session(&session_id(5)));
}

#[test]
fn explicit_close_is_the_only_normal_way_to_free_capacity() {
    let mut broker = broker_with_sessions(4);

    let removed = broker.close_session(&session_id(2)).unwrap();
    assert_eq!(removed.session_id, session_id(2));
    broker.open_session(session(5)).unwrap();

    assert_eq!(broker.len(), 4);
    assert!(!broker.contains_session(&session_id(2)));
    assert!(broker.contains_session(&session_id(1)));
    assert!(broker.contains_session(&session_id(3)));
    assert!(broker.contains_session(&session_id(4)));
    assert!(broker.contains_session(&session_id(5)));
}

#[test]
fn only_one_foreground_lease_exists_and_switch_is_explicit() {
    let mut broker = broker_with_sessions(3);
    assert!(broker.foreground_output().is_none());

    let first = broker.switch_foreground(&session_id(1)).unwrap();
    let idempotent = broker.switch_foreground(&session_id(1)).unwrap();
    assert_eq!(first, idempotent);
    assert_eq!(broker.foreground_output(), Some(&first));

    let second = broker.switch_foreground(&session_id(2)).unwrap();
    assert_eq!(second.session_id, session_id(2));
    assert!(second.generation > first.generation);
    assert_eq!(broker.foreground_output(), Some(&second));
    assert_eq!(broker.snapshot().foreground_output, Some(second));
}

#[test]
fn capture_pin_rejects_switch_clear_and_owner_close_until_released() {
    let mut broker = broker_with_sessions(3);
    let lease = broker.switch_foreground(&session_id(1)).unwrap();
    let pin = broker
        .pin_foreground(&session_id(1), OutputPinKind::Capture)
        .unwrap();

    assert_eq!(pin.session_id(), &session_id(1));
    assert_eq!(pin.lease_generation(), lease.generation);
    assert_eq!(pin.kind(), OutputPinKind::Capture);
    assert_eq!(
        broker.switch_foreground(&session_id(2)).unwrap_err(),
        BrokerError::SessionOutputLeasePinned
    );
    assert_eq!(
        broker.clear_foreground().unwrap_err(),
        BrokerError::SessionOutputLeasePinned
    );
    assert_eq!(
        broker.close_session(&session_id(1)).unwrap_err(),
        BrokerError::SessionOutputLeasePinned
    );

    assert_eq!(broker.switch_foreground(&session_id(1)).unwrap(), lease);
    broker.close_session(&session_id(3)).unwrap();
    broker.release_output_pin(&pin).unwrap();
    let switched = broker.switch_foreground(&session_id(2)).unwrap();
    assert_eq!(switched.session_id, session_id(2));
}

#[test]
fn mp4_pin_has_the_same_exclusive_switch_rule() {
    let mut broker = broker_with_sessions(2);
    broker.switch_foreground(&session_id(1)).unwrap();
    let pin = broker
        .pin_foreground(&session_id(1), OutputPinKind::Mp4)
        .unwrap();

    assert_eq!(pin.kind(), OutputPinKind::Mp4);
    assert_eq!(
        broker.switch_foreground(&session_id(2)).unwrap_err(),
        BrokerError::SessionOutputLeasePinned
    );
    assert_eq!(
        broker
            .pin_foreground(&session_id(1), OutputPinKind::Capture)
            .unwrap_err(),
        BrokerError::SessionOutputLeasePinned
    );
}

#[test]
fn stale_or_foreign_pin_token_cannot_release_current_pin() {
    let mut broker = broker_with_sessions(1);
    broker.switch_foreground(&session_id(1)).unwrap();
    let old = broker
        .pin_foreground(&session_id(1), OutputPinKind::Capture)
        .unwrap();
    broker.release_output_pin(&old).unwrap();
    let current = broker
        .pin_foreground(&session_id(1), OutputPinKind::Capture)
        .unwrap();

    assert!(current.pin_generation() > old.pin_generation());
    assert_eq!(
        broker.release_output_pin(&old).unwrap_err(),
        BrokerError::OutputPinMismatch
    );
    assert_eq!(broker.output_pin(), Some(&current));
    broker.release_output_pin(&current).unwrap();
    assert!(broker.output_pin().is_none());
}

#[test]
fn pin_requires_the_calling_session_to_own_foreground() {
    let mut broker = broker_with_sessions(2);
    broker.switch_foreground(&session_id(1)).unwrap();

    assert_eq!(
        broker
            .pin_foreground(&session_id(2), OutputPinKind::Capture)
            .unwrap_err(),
        BrokerError::SessionDoesNotOwnForeground
    );
}

#[test]
fn fault_of_background_worker_removes_only_its_session() {
    let mut broker = broker_with_sessions(4);
    let foreground = broker.switch_foreground(&session_id(1)).unwrap();
    let pin = broker
        .pin_foreground(&session_id(1), OutputPinKind::Capture)
        .unwrap();

    let removed = broker.handle_worker_fault(&worker_id(3)).unwrap();

    assert_eq!(removed.session_id, session_id(3));
    assert_eq!(broker.len(), 3);
    assert!(broker.contains_session(&session_id(1)));
    assert!(broker.contains_session(&session_id(2)));
    assert!(!broker.contains_session(&session_id(3)));
    assert!(broker.contains_session(&session_id(4)));
    assert_eq!(broker.foreground_output(), Some(&foreground));
    assert_eq!(broker.output_pin(), Some(&pin));
}

#[test]
fn fault_of_foreground_worker_force_releases_its_lease_and_pin_only() {
    let mut broker = broker_with_sessions(3);
    broker.switch_foreground(&session_id(1)).unwrap();
    broker
        .pin_foreground(&session_id(1), OutputPinKind::Mp4)
        .unwrap();

    let removed = broker.handle_worker_fault(&worker_id(1)).unwrap();

    assert_eq!(removed.session_id, session_id(1));
    assert!(!broker.contains_session(&session_id(1)));
    assert!(broker.contains_session(&session_id(2)));
    assert!(broker.contains_session(&session_id(3)));
    assert!(broker.foreground_output().is_none());
    assert!(broker.output_pin().is_none());
}

#[test]
fn fault_frees_capacity_without_selecting_or_eviction() {
    let mut broker = broker_with_sessions(4);
    broker.handle_worker_fault(&worker_id(2)).unwrap();
    broker.open_session(session(5)).unwrap();

    assert_eq!(broker.len(), 4);
    assert!(!broker.contains_session(&session_id(2)));
    assert!(broker.contains_session(&session_id(5)));
    assert!(broker.foreground_output().is_none());
}

#[test]
fn duplicate_session_and_worker_identity_are_rejected_deterministically() {
    let mut broker = broker_with_sessions(1);
    assert_eq!(
        broker.open_session(session(1)).unwrap_err(),
        BrokerError::SessionAlreadyExists
    );

    let mut duplicate_worker = session(2);
    duplicate_worker.worker_id = worker_id(1);
    assert_eq!(
        broker.open_session(duplicate_worker).unwrap_err(),
        BrokerError::WorkerAlreadyAssigned
    );
    assert_eq!(broker.len(), 1);
}

#[test]
fn unknown_session_and_worker_operations_leave_state_unchanged() {
    let mut broker = broker_with_sessions(1);
    let before = broker.snapshot();

    assert_eq!(
        broker.close_session(&session_id(9)).unwrap_err(),
        BrokerError::SessionNotFound
    );
    assert_eq!(
        broker.switch_foreground(&session_id(9)).unwrap_err(),
        BrokerError::SessionNotFound
    );
    assert_eq!(
        broker.handle_worker_fault(&worker_id(9)).unwrap_err(),
        BrokerError::WorkerNotFound
    );
    assert_eq!(broker.snapshot(), before);
}

#[test]
fn session_inventory_is_sorted_by_exact_case_sensitive_id() {
    let mut broker = SessionBroker::new();
    for number in [3, 1, 4, 2] {
        broker.open_session(session(number)).unwrap();
    }

    let ids = broker
        .sessions()
        .map(|session| session.session_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["session-1", "session-2", "session-3", "session-4"]);
    assert!(!broker.is_empty());
}

#[test]
fn closing_unpinned_foreground_clears_only_the_lease() {
    let mut broker = broker_with_sessions(2);
    broker.switch_foreground(&session_id(1)).unwrap();

    broker.close_session(&session_id(1)).unwrap();

    assert!(broker.foreground_output().is_none());
    assert!(broker.contains_session(&session_id(2)));
    assert_eq!(broker.len(), 1);
}
