//! Hosted control plane. Runtime history stays with the execution engine.
#[cfg(test)]
mod aaa_sql_census;
/// Tests that need the literal `zuse_staging` schema (the code checks for
/// it) take turns: one holds this while it recreates and uses the schema.
#[cfg(test)]
pub(crate) static STAGING_SCHEMA_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub mod access;
#[cfg(test)]
mod zzz_sql_census {
    #[test]
    fn report() {
        crate::aaa_sql_census::report();
    }
}
pub mod archive;
pub mod auth;
pub mod computers;
pub mod config;
pub mod connections;
pub mod crypto;
pub mod device_proof;
pub mod devices;
pub mod error;
pub mod events;
pub mod host_link;
pub mod jobs;
pub mod managed_bases;
pub mod model_catalog;
pub mod project_caches;
pub mod providers;
pub mod routes;
pub mod runtime;
pub mod runtime_releases;
pub mod store;
pub mod tickets;
pub mod tunnels;

pub const REFERENCE_COMMIT: &str = "2016cf4f0cc240e6797916dd5cab1d8bebab89ae";

pub mod gateway;

pub mod billing;
pub mod certification;

pub mod objects;
pub mod transcript;

pub mod projects;

pub mod github;

pub mod images;

#[cfg(feature = "hosted-browser")]
pub mod hosted_browser;
pub mod plugins;
pub mod slack;
