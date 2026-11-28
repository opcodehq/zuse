//! Cached catalog probes for hot paths.
//!
//! Request handlers and worker ticks used to ask PostgreSQL "does this table
//! exist?" on every call. Migrations only ever add these objects, so a probe
//! that once saw them present is cached for the life of the pool. An absent
//! answer (an optional migration not installed yet) is re-checked at most
//! once per [`ABSENT_RECHECK`], so installing it later still takes effect
//! without a restart. Startup readiness checks keep querying directly.
use sqlx::PgPool;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

/// How long an "absent" answer is trusted before it is probed again.
#[cfg(not(test))]
pub const ABSENT_RECHECK: Duration = Duration::from_secs(60);
/// Unit tests create schemas mid-test; never cache absence there.
#[cfg(test)]
pub const ABSENT_RECHECK: Duration = Duration::ZERO;

struct Entry {
    /// Ties the entry to one live pool: a dropped pool can never alias a new one.
    pool: Weak<sqlx::postgres::PgConnectOptions>,
    present: bool,
    checked: Instant,
}

type Key = (usize, &'static str);
static CACHE: Mutex<Option<HashMap<Key, Entry>>> = Mutex::new(None);

fn identity(pool: &PgPool) -> (usize, Arc<sqlx::postgres::PgConnectOptions>) {
    let options = pool.connect_options();
    (Arc::as_ptr(&options) as usize, options)
}

fn cached(key: Key, options: &Arc<sqlx::postgres::PgConnectOptions>) -> Option<bool> {
    let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let entry = cache.as_ref()?.get(&key)?;
    let same_pool = entry
        .pool
        .upgrade()
        .is_some_and(|live| Arc::ptr_eq(&live, options));
    if !same_pool {
        return None;
    }
    if entry.present || entry.checked.elapsed() < ABSENT_RECHECK {
        return Some(entry.present);
    }
    None
}

/// Runs `sql` (a single boolean catalog probe) at most once per pool while it
/// reports `true`.
pub async fn present(pool: &PgPool, sql: &'static str) -> Result<bool, sqlx::Error> {
    let (address, options) = identity(pool);
    let key = (address, sql);
    if let Some(present) = cached(key, &options) {
        return Ok(present);
    }
    let present: bool = sqlx::query_scalar(sql).fetch_one(pool).await?;
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    // Drop entries of pools that no longer exist so the map stays bounded.
    cache.retain(|_, entry| entry.pool.strong_count() > 0);
    cache.insert(
        key,
        Entry {
            pool: Arc::downgrade(&options),
            present,
            checked: Instant::now(),
        },
    );
    Ok(present)
}

/// Catalog probe for a borrowed connection or transaction. Its pool and schema
/// are unknown, so a result cannot safely enter the shared cache. Integration
/// tests also link this function without `cfg(test)` and use isolated schemas.
pub async fn present_in(
    connection: &mut sqlx::PgConnection,
    sql: &'static str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(sql).fetch_one(connection).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn present_answers_are_cached_per_pool() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let (address, options) = identity(&pool);
        let key = (address, "SELECT true");
        assert_eq!(cached(key, &options), None);
        CACHE
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(
                key,
                Entry {
                    pool: Arc::downgrade(&options),
                    present: true,
                    checked: Instant::now(),
                },
            );
        // No query is issued: the lazy pool points at a closed port.
        assert!(present(&pool, "SELECT true").await.unwrap());
        assert!(pool.clone().connect_options().as_ref() as *const _ as usize == address);
        // Another pool with identical options does not share the entry.
        let other = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        let (other_address, other_options) = identity(&other);
        assert_eq!(cached((other_address, "SELECT true"), &other_options), None);
        // Absence is never cached in unit tests (ABSENT_RECHECK is zero).
        CACHE.lock().unwrap().as_mut().unwrap().insert(
            (address, "SELECT false"),
            Entry {
                pool: Arc::downgrade(&options),
                present: false,
                checked: Instant::now(),
            },
        );
        assert_eq!(cached((address, "SELECT false"), &options), None);
    }
}
