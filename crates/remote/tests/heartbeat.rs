//! Shared monotonic liveness state; no wall-clock sleeps or transport pongs.
use zuse_remote::heartbeat::{self, Action, Heartbeat};
fn probe(state: &mut Heartbeat, now: u64) -> Vec<u8> {
    match state.poll(now).unwrap() {
        Action::Send(payload) => payload,
        _ => panic!("expected challenge"),
    }
}
#[test]
fn only_timely_challenge_replies_advance_liveness() {
    let mut state = Heartbeat::new();
    let first = probe(&mut state, 0);
    assert_eq!(
        heartbeat::response(br#"{"id":1,"ok":{"text":"alive"}}"#).unwrap(),
        None
    );
    assert!(!state.consume(br#"{"id":1,"ok":true}"#, 9999).unwrap());
    assert!(matches!(state.poll(9999).unwrap(), Action::Wait(1)));
    assert!(matches!(state.poll(10_000).unwrap(), Action::Expired));
    assert!(
        state
            .consume(&heartbeat::response(&first).unwrap().unwrap(), 10_000)
            .is_err()
    );
    let mut state = Heartbeat::new();
    let first = probe(&mut state, 0);
    let response = heartbeat::response(&first).unwrap().unwrap();
    assert!(state.consume(&response, 10).unwrap());
    assert!(matches!(state.poll(20_009).unwrap(), Action::Wait(1)));
    let next = probe(&mut state, 20_010);
    assert_ne!(first, next, "every challenge is fresh");
    assert!(
        state.consume(&response, 20_011).is_err(),
        "old reply cannot prove a new challenge"
    );
    assert!(!state.consume(b"pong", 79_000).unwrap());
    assert!(matches!(state.poll(80_010).unwrap(), Action::Expired));
}
#[test]
fn controls_reject_reflection_corruption_and_unsolicited_replies() {
    let mut state = Heartbeat::new();
    let request = probe(&mut state, 0);
    assert!(
        state.consume(&request, 1).is_err(),
        "request reflection is not a reply"
    );
    let mut reply = heartbeat::response(&request).unwrap().unwrap();
    assert!(
        heartbeat::response(&reply).is_err(),
        "host rejects reflected responses"
    );
    assert!(Heartbeat::new().consume(&reply, 1).is_err());
    *reply.last_mut().unwrap() ^= 1;
    assert!(state.consume(&reply, 1).is_err());
    reply.pop();
    assert!(state.consume(&reply, 1).is_err());
    assert!(heartbeat::response(&request[..request.len() - 1]).is_err());
}
