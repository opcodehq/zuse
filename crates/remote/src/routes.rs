//! Every way of reaching one paired host, and the choice between them.
//!
//! Noise authenticates the host and encrypts the session end to end, so a
//! route is only a pipe: the wrong machine on a stale LAN address fails the
//! handshake without learning anything. Advertised addresses are hints, and
//! only the connecting device can prove one works, so choices here are driven
//! by measurements the caller supplies. Time is elapsed milliseconds.
use crate::Error;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;

/// A candidate must win by both margins to replace a route of the same class.
pub const SWITCH_RATIO: f64 = 0.7;
pub const SWITCH_MARGIN_MS: u32 = 15;
pub const SWITCH_STREAK: u8 = 2;
/// A switch rests the selector, so a noisy network cannot bounce the session.
pub const SWITCH_REST_MS: u64 = 30_000;
/// A route that answered probes but then failed to take over the session.
pub const FAILED_TAKEOVER_COOLDOWN_MS: u64 = 300_000;
pub const PROBE_INTERVAL_MS: u64 = 60_000;
pub const PROBE_CONFIRM_MS: u64 = 3_000;
const PROBE_BACKOFF_CAP_MS: u64 = 900_000;
const RTT_WEIGHT: f64 = 0.3;
const MAX_ROUTES: usize = 16;
const MAX_NETWORKS: usize = 16;

/// Declared in preference order: within the switching margin, the earlier
/// kind wins because it has fewer hops and costs less battery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteKind {
    Loopback,
    Lan,
    Tailnet,
    Public,
    Relay,
}
impl RouteKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Loopback => "This computer",
            Self::Lan => "Local network",
            Self::Tailnet => "Tailscale",
            Self::Public => "Direct",
            Self::Relay => "Relay",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Route {
    pub kind: RouteKind,
    pub origin: String,
}
impl Route {
    /// True when a browser served over HTTPS may open this route.
    pub fn is_secure(&self) -> bool {
        self.origin.starts_with("https:")
    }
}

/// Addresses only reachable from inside a private network.
pub fn is_private_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || is_shared_space(ip)
        }
        Ok(IpAddr::V6(ip)) => {
            let first = ip.segments()[0];
            ip.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
        Err(_) => false,
    }
}
fn is_shared_space(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (64..128).contains(&b)
}
/// The tailnet's IPv4 range is carrier-grade NAT space other VPNs share too,
/// so hosts confirm it against the interface before advertising it.
pub fn is_tailnet_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.to_ascii_lowercase().ends_with(".ts.net") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => is_shared_space(ip),
        Ok(IpAddr::V6(ip)) => {
            let [a, b, c, ..] = ip.segments();
            (a, b, c) == (0xfd7a, 0x115c, 0xa1e0)
        }
        Err(_) => false,
    }
}

/// Plain HTTP is accepted for private addresses because a paired route carries
/// only Noise records. Application bundles still need [`crate::normalized_origin`].
pub fn normalized_route_origin(origin: &str) -> Result<String, Error> {
    let mut url = url::Url::parse(origin).map_err(|_| Error::Endpoint)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Endpoint);
    }
    let scheme = match url.scheme() {
        "https" | "wss" => "https",
        "http" | "ws" => "http",
        _ => return Err(Error::Endpoint),
    };
    if scheme == "http" && !is_private_host(url.host_str().unwrap_or_default()) {
        return Err(Error::Endpoint);
    }
    url.set_scheme(scheme).map_err(|_| Error::Endpoint)?;
    Ok(url.origin().ascii_serialization())
}

/// Classify a route from its address alone, for origins nothing else describes.
pub fn classify_origin(origin: &str) -> RouteKind {
    let Some(host) = url::Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
    else {
        return RouteKind::Public;
    };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = bare.eq_ignore_ascii_case("localhost")
        || bare.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if loopback {
        RouteKind::Loopback
    } else if is_tailnet_host(&host) {
        RouteKind::Tailnet
    } else if is_private_host(&host) {
        RouteKind::Lan
    } else {
        RouteKind::Public
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Stay,
    /// Faster so far; measure once more shortly before moving the session.
    Confirm,
    Switch,
}

#[derive(Clone, Debug)]
struct Entry {
    route: Route,
    /// Invitation routes stay; learned routes follow what the host reports.
    learned: bool,
    rtt_ms: Option<f64>,
    streak: u8,
    failures: u32,
    cooldown_until: u64,
    next_probe_at: u64,
}
impl Entry {
    fn new(route: Route, learned: bool) -> Self {
        Self {
            route,
            learned,
            rtt_ms: None,
            streak: 0,
            failures: 0,
            cooldown_until: 0,
            next_probe_at: 0,
        }
    }
}

/// Non-secret route memory, safe to keep beside ordinary settings.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedRoutes {
    pub learned: Vec<Route>,
    pub best_by_network: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct RouteTable {
    entries: Vec<Entry>,
    network: String,
    best_by_network: HashMap<String, String>,
    rest_until: u64,
}
impl RouteTable {
    pub fn new(invited: Route) -> Self {
        Self {
            entries: vec![Entry::new(invited, false)],
            network: String::new(),
            best_by_network: HashMap::new(),
            rest_until: 0,
        }
    }
    pub fn restore(&mut self, saved: SavedRoutes) {
        self.best_by_network = saved.best_by_network;
        self.best_by_network.truncate_to(MAX_NETWORKS);
        self.learn(saved.learned);
    }
    pub fn saved(&self) -> SavedRoutes {
        SavedRoutes {
            learned: self
                .entries
                .iter()
                .filter(|entry| entry.learned)
                .map(|entry| entry.route.clone())
                .collect(),
            best_by_network: self.best_by_network.clone(),
        }
    }
    pub fn routes(&self) -> Vec<Route> {
        self.entries.iter().map(|e| e.route.clone()).collect()
    }
    /// Replace learned routes with the host's current report, so a changed
    /// address replaces the old one. Measurements of surviving routes stay.
    pub fn learn(&mut self, reported: Vec<Route>) -> bool {
        let before = self.routes();
        self.entries
            .retain(|entry| !entry.learned || reported.contains(&entry.route));
        for route in reported {
            if normalized_route_origin(&route.origin).ok().as_deref() != Some(route.origin.as_str())
            {
                continue;
            }
            if self.entries.len() < MAX_ROUTES
                && !self.entries.iter().any(|e| e.route.origin == route.origin)
            {
                self.entries.push(Entry::new(route, true));
            }
        }
        before != self.routes()
    }
    fn entry_mut(&mut self, route: &Route) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.route == *route)
    }
    fn entry(&self, route: &Route) -> Option<&Entry> {
        self.entries.iter().find(|e| e.route == *route)
    }
    /// Measurements describe the path, so they die with it. Returns true when
    /// the fingerprint actually changed.
    pub fn network_changed(&mut self, network: &str) -> bool {
        let changed = self.network != network;
        self.network = network.into();
        for entry in &mut self.entries {
            entry.rtt_ms = None;
            entry.streak = 0;
            entry.failures = 0;
            entry.cooldown_until = 0;
            entry.next_probe_at = 0;
        }
        self.rest_until = 0;
        changed
    }
    /// Last winner on this network first, so a known place connects at once.
    pub fn dial_order(&self, secure_only: bool) -> Vec<Route> {
        let best = self.best_by_network.get(&self.network);
        let mut routes: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| !secure_only || entry.route.is_secure())
            .collect();
        routes.sort_by_key(|entry| (Some(&entry.route.origin) != best, entry.route.kind));
        routes.into_iter().map(|e| e.route.clone()).collect()
    }
    pub fn record_rtt(&mut self, route: &Route, rtt_ms: u32) {
        if let Some(entry) = self.entry_mut(route) {
            let sample = f64::from(rtt_ms);
            entry.rtt_ms = Some(match entry.rtt_ms {
                Some(previous) => previous + RTT_WEIGHT * (sample - previous),
                None => sample,
            });
        }
    }
    pub fn rtt(&self, route: &Route) -> Option<u32> {
        self.entry(route)?.rtt_ms.map(|rtt| rtt.round() as u32)
    }
    pub fn record_connected(&mut self, route: &Route, now: u64) {
        let network = self.network.clone();
        if let Some(entry) = self.entry_mut(route) {
            entry.failures = 0;
            entry.streak = 0;
            entry.next_probe_at = now + PROBE_INTERVAL_MS;
        }
        self.best_by_network.insert(network, route.origin.clone());
        self.best_by_network.truncate_to(MAX_NETWORKS);
    }
    pub fn record_failure(&mut self, route: &Route, now: u64) {
        if let Some(entry) = self.entry_mut(route) {
            entry.failures = entry.failures.saturating_add(1);
            entry.streak = 0;
            let backoff = PROBE_INTERVAL_MS
                .saturating_mul(1 << entry.failures.min(8))
                .min(PROBE_BACKOFF_CAP_MS);
            entry.next_probe_at = now + backoff;
        }
    }
    pub fn record_failed_takeover(&mut self, route: &Route, now: u64) {
        if let Some(entry) = self.entry_mut(route) {
            entry.streak = 0;
            entry.cooldown_until = now + FAILED_TAKEOVER_COOLDOWN_MS;
        }
    }
    pub fn record_switched(&mut self, route: &Route, now: u64) {
        self.record_connected(route, now);
        self.rest_until = now + SWITCH_REST_MS;
    }
    /// Probes due now. Nothing beats a healthy LAN, and a route already under
    /// the switching margin can only lose to a better class of route.
    pub fn probes_due(&self, current: &Route, now: u64, secure_only: bool) -> Vec<Route> {
        if current.kind <= RouteKind::Lan || now < self.rest_until {
            return vec![];
        }
        let current_rtt = self.rtt(current);
        self.entries
            .iter()
            .filter(|entry| entry.route != *current)
            .filter(|entry| !secure_only || entry.route.is_secure())
            .filter(|entry| now >= entry.cooldown_until && now >= entry.next_probe_at)
            .filter(|entry| {
                entry.route.kind < current.kind
                    || current_rtt.is_none_or(|rtt| rtt > SWITCH_MARGIN_MS)
            })
            .map(|entry| entry.route.clone())
            .collect()
    }
    /// Record one successful probe and decide whether the session should move.
    pub fn judge_probe(
        &mut self,
        current: &Route,
        candidate: &Route,
        rtt_ms: u32,
        now: u64,
    ) -> Decision {
        self.record_rtt(candidate, rtt_ms);
        let current_rtt = self.rtt(current);
        // A smoothed average would hide a bad sample behind earlier good ones.
        let candidate_rtt = self.rtt(candidate).unwrap_or(rtt_ms).max(rtt_ms);
        let resting = now < self.rest_until;
        let Some(entry) = self.entry_mut(candidate) else {
            return Decision::Stay;
        };
        entry.failures = 0;
        let wins = !resting && beats(current.kind, current_rtt, candidate.kind, candidate_rtt);
        if !wins {
            entry.streak = 0;
            entry.next_probe_at = now + PROBE_INTERVAL_MS;
            return Decision::Stay;
        }
        entry.streak += 1;
        if entry.streak >= SWITCH_STREAK {
            Decision::Switch
        } else {
            entry.next_probe_at = now + PROBE_CONFIRM_MS;
            Decision::Confirm
        }
    }
}

fn beats(current: RouteKind, current_rtt: Option<u32>, candidate: RouteKind, rtt: u32) -> bool {
    let Some(now) = current_rtt else {
        return candidate < current;
    };
    let clearly_faster =
        rtt + SWITCH_MARGIN_MS <= now && f64::from(rtt) <= f64::from(now) * SWITCH_RATIO;
    let clearly_slower =
        now + SWITCH_MARGIN_MS <= rtt && f64::from(now) <= f64::from(rtt) * SWITCH_RATIO;
    clearly_faster || (candidate < current && !clearly_slower)
}

trait Bounded {
    fn truncate_to(&mut self, limit: usize);
}
impl Bounded for HashMap<String, String> {
    fn truncate_to(&mut self, limit: usize) {
        while self.len() > limit {
            let Some(key) = self.keys().next().cloned() else {
                return;
            };
            self.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn route(kind: RouteKind, origin: &str) -> Route {
        Route {
            kind,
            origin: origin.into(),
        }
    }
    fn lan() -> Route {
        route(RouteKind::Lan, "http://192.168.1.5:27657")
    }
    fn tailnet() -> Route {
        route(RouteKind::Tailnet, "http://100.100.1.1:27657")
    }
    fn relay() -> Route {
        route(RouteKind::Relay, "https://relay.example.com")
    }
    fn table() -> RouteTable {
        let mut table = RouteTable::new(relay());
        table.learn(vec![lan(), tailnet()]);
        table
    }

    #[test]
    fn plain_http_is_only_for_private_addresses() {
        for origin in [
            "http://192.168.1.5:27657",
            "http://100.100.1.1:27657",
            "http://[fd7a:115c:a1e0::1]:27657",
            "http://localhost:1",
        ] {
            assert!(normalized_route_origin(origin).is_ok(), "{origin}");
        }
        for origin in ["http://8.8.8.8:80", "http://example.com", "ftp://10.0.0.1"] {
            assert!(normalized_route_origin(origin).is_err(), "{origin}");
        }
        assert!(normalized_route_origin("https://example.com").is_ok());
    }

    #[test]
    fn classifies_addresses() {
        assert_eq!(classify_origin("http://127.0.0.1:1"), RouteKind::Loopback);
        assert_eq!(classify_origin("http://10.1.2.3:1"), RouteKind::Lan);
        assert_eq!(classify_origin("http://100.64.0.9:1"), RouteKind::Tailnet);
        assert_eq!(
            classify_origin("https://box.tail1234.ts.net"),
            RouteKind::Tailnet
        );
        assert_eq!(classify_origin("https://zuse.dev"), RouteKind::Public);
    }

    #[test]
    fn learning_follows_the_host_but_keeps_the_invitation() {
        let mut table = table();
        let moved = route(RouteKind::Lan, "http://192.168.9.9:27657");
        assert!(table.learn(vec![moved.clone()]));
        assert_eq!(table.routes(), vec![relay(), moved]);
        assert!(!table.learn(table.saved().learned));
    }

    #[test]
    fn learning_refuses_public_plain_http() {
        let mut table = RouteTable::new(relay());
        table.learn(vec![route(RouteKind::Lan, "http://8.8.8.8:1")]);
        assert_eq!(table.routes(), vec![relay()]);
    }

    #[test]
    fn dial_order_prefers_the_last_winner_on_this_network() {
        let mut table = table();
        table.network_changed("home");
        assert_eq!(table.dial_order(false)[0], lan());
        table.record_connected(&tailnet(), 0);
        assert_eq!(table.dial_order(false)[0], tailnet());
        table.network_changed("cafe");
        assert_eq!(table.dial_order(false)[0], lan());
    }

    #[test]
    fn browsers_only_see_secure_routes() {
        assert_eq!(table().dial_order(true), vec![relay()]);
    }

    #[test]
    fn moving_needs_a_clear_win_measured_twice() {
        let mut table = table();
        table.record_rtt(&relay(), 120);
        assert_eq!(
            table.judge_probe(&relay(), &tailnet(), 40, 0),
            Decision::Confirm
        );
        assert_eq!(
            table.judge_probe(&relay(), &tailnet(), 40, 3_000),
            Decision::Switch
        );
    }

    #[test]
    fn a_slow_tailnet_does_not_replace_a_faster_relay() {
        let mut table = table();
        table.record_rtt(&relay(), 40);
        assert_eq!(
            table.judge_probe(&relay(), &tailnet(), 180, 0),
            Decision::Stay
        );
    }

    #[test]
    fn a_faster_relay_replaces_a_tailnet_stuck_on_a_slow_path() {
        let mut table = table();
        table.record_rtt(&tailnet(), 220);
        assert_eq!(
            table.judge_probe(&tailnet(), &relay(), 60, 0),
            Decision::Confirm
        );
        assert_eq!(
            table.judge_probe(&tailnet(), &relay(), 60, 3_000),
            Decision::Switch
        );
    }

    #[test]
    fn equal_speed_prefers_the_better_class_without_reversing() {
        let mut table = table();
        table.record_rtt(&tailnet(), 12);
        assert_eq!(
            table.judge_probe(&tailnet(), &lan(), 14, 0),
            Decision::Confirm
        );
        let mut settled = self::table();
        settled.record_rtt(&lan(), 14);
        assert_eq!(
            settled.judge_probe(&lan(), &tailnet(), 12, 0),
            Decision::Stay
        );
    }

    #[test]
    fn a_noisy_sample_resets_the_streak() {
        let mut table = table();
        table.record_rtt(&relay(), 120);
        assert_eq!(
            table.judge_probe(&relay(), &tailnet(), 40, 0),
            Decision::Confirm
        );
        assert_eq!(
            table.judge_probe(&relay(), &tailnet(), 400, 1),
            Decision::Stay
        );
    }

    #[test]
    fn switching_rests_the_selector() {
        let mut table = table();
        table.record_switched(&tailnet(), 1_000);
        assert!(table.probes_due(&tailnet(), 2_000, false).is_empty());
        assert_eq!(
            table.judge_probe(&tailnet(), &relay(), 1, 2_000),
            Decision::Stay
        );
    }

    #[test]
    fn lan_stops_probing_and_quiet_tailnets_only_look_up() {
        let mut table = table();
        assert!(table.probes_due(&lan(), 0, false).is_empty());
        table.record_rtt(&tailnet(), 8);
        assert_eq!(table.probes_due(&tailnet(), 0, false), vec![lan()]);
    }

    #[test]
    fn silent_routes_back_off_and_network_changes_clear_it() {
        let mut table = table();
        table.record_failure(&lan(), 0);
        assert!(!table.probes_due(&relay(), 1_000, false).contains(&lan()));
        table.network_changed("elsewhere");
        assert!(table.probes_due(&relay(), 1_000, false).contains(&lan()));
    }

    #[test]
    fn failed_takeover_cools_down() {
        let mut table = table();
        table.record_failed_takeover(&lan(), 0);
        assert!(!table.probes_due(&relay(), 60_000, false).contains(&lan()));
        assert!(table.probes_due(&relay(), 300_000, false).contains(&lan()));
    }

    #[test]
    fn saved_routes_restore_without_secrets() {
        let mut first = table();
        first.network_changed("home");
        first.record_connected(&tailnet(), 0);
        let text = serde_json::to_string(&first.saved()).unwrap();
        let mut second = RouteTable::new(relay());
        second.restore(serde_json::from_str(&text).unwrap());
        second.network_changed("home");
        assert_eq!(second.dial_order(false)[0], tailnet());
    }
}
