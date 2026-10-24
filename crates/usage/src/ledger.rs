//! Preserve normalized history in the application's own profile after CLI exports are pruned.
use crate::{Data, Record};
use rusqlite::{Connection, params};
use std::{collections::BTreeMap, path::Path};

pub struct Ledger {
    db: Connection,
}
impl Ledger {
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(2))?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS usage_records (id TEXT PRIMARY KEY NOT NULL, record_json TEXT NOT NULL);")?;
        Ok(Self { db })
    }
    pub fn merge(&mut self, mut scanned: Data) -> Result<Data, rusqlite::Error> {
        let mut records: BTreeMap<String, Record> = BTreeMap::new();
        {
            let mut statement = self
                .db
                .prepare("SELECT id,record_json FROM usage_records ORDER BY id")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let json: String = row.get(1)?;
                let record = serde_json::from_str::<Record>(&json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                records.insert(row.get(0)?, record);
            }
        }
        let transaction = self.db.transaction()?;
        for mut record in scanned.records {
            if let Some(previous) = records.get(&record.id) {
                if previous.tokens.total() > record.tokens.total() {
                    continue;
                }
                if record.cost_usd.is_none() {
                    record.cost_usd = previous.cost_usd;
                }
            }
            let json = serde_json::to_string(&record)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            transaction.execute("INSERT INTO usage_records(id,record_json) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET record_json=excluded.record_json",params![record.id,json])?;
            records.insert(record.id.clone(), record);
        }
        transaction.commit()?;
        scanned.records = records.into_values().collect();
        scanned.deduplicate();
        for status in &mut scanned.sources {
            status.record_count = scanned
                .records
                .iter()
                .filter(|r| r.source_id == status.id)
                .count();
        }
        Ok(scanned)
    }
}
