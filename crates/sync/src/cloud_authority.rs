//! A cloud host accepts execution only from its authenticated API mailbox.
//! Peers can mirror a command document, but cannot write this local proof.
use super::*;
impl DocsStore {
    pub fn authorize_cloud_command(
        &self,
        doc: &str,
        command: &str,
        digest: &str,
    ) -> Result<(), StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            conn.execute("INSERT INTO cloud_command_authorizations(doc_id,command_id,digest) VALUES(?1,?2,?3) ON CONFLICT(doc_id,command_id) DO NOTHING",params![doc,command,digest])?;
            let saved: String = conn.query_row(
                "SELECT digest FROM cloud_command_authorizations WHERE doc_id=?1 AND command_id=?2",
                params![doc, command],
                |r| r.get(0),
            )?;
            if saved != digest {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "cloud_command_authority_conflict",
                )
                .into());
            }
            Ok(())
        })
    }
    pub fn authorize_cloud_interrupt(
        &self,
        doc: &str,
        command: &str,
        target: Option<(&str, &str)>,
    ) -> Result<(), StoreError> {
        let key = serde_json::to_string(&("cloud-interrupt-v1", command))
            .map_err(std::io::Error::other)?;
        let target = serde_json::to_string(&target).map_err(std::io::Error::other)?;
        self.authorize_cloud_command(doc, &key, &target)
    }
    pub fn cloud_interrupt_target(
        &self,
        doc: &str,
        command: &str,
    ) -> Result<Option<String>, StoreError> {
        let key = serde_json::to_string(&("cloud-interrupt-v1", command))
            .map_err(std::io::Error::other)?;
        store_blocking(|| {
            Ok(self.conn().query_row("SELECT digest FROM cloud_command_authorizations WHERE doc_id=?1 AND command_id=?2",params![doc,key],|r|r.get(0)).optional()?)
        })
    }
    pub fn stop_cloud_run(
        &self,
        doc: &str,
        command: &str,
        message: &str,
    ) -> Result<(), StoreError> {
        let key = serde_json::to_string(&("cloud-stopped-run-v1", command))
            .map_err(std::io::Error::other)?;
        self.authorize_cloud_command(doc, &key, message)
    }
    pub fn cloud_run_stopped(
        &self,
        doc: &str,
        command: &str,
        message: &str,
    ) -> Result<bool, StoreError> {
        let key = serde_json::to_string(&("cloud-stopped-run-v1", command))
            .map_err(std::io::Error::other)?;
        self.cloud_command_authorized(doc, &key, message)
    }
    pub fn cloud_command_authorized(
        &self,
        doc: &str,
        command: &str,
        digest: &str,
    ) -> Result<bool, StoreError> {
        store_blocking(|| {
            Ok(self.conn().query_row("SELECT digest FROM cloud_command_authorizations WHERE doc_id=?1 AND command_id=?2",params![doc,command],|r|r.get::<_,String>(0)).optional()?.is_some_and(|saved|saved==digest))
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cloud_authority_is_durable_immutable_and_document_scoped() {
        let root = tempfile::tempdir().unwrap();
        let store = DocsStore::open_durable(root.path()).unwrap();
        assert!(
            !store
                .cloud_command_authorized("task", "command", "digest")
                .unwrap()
        );
        store
            .authorize_cloud_command("task", "command", "digest")
            .unwrap();
        store
            .authorize_cloud_command("task", "command", "digest")
            .unwrap();
        assert!(
            store
                .authorize_cloud_command("task", "command", "changed")
                .is_err()
        );
        assert!(
            !store
                .cloud_command_authorized("other", "command", "digest")
                .unwrap()
        );
        drop(store);
        let store = DocsStore::open_durable(root.path()).unwrap();
        assert!(
            store
                .cloud_command_authorized("task", "command", "digest")
                .unwrap()
        );
        store.delete_snapshot("task").unwrap();
        assert!(
            !store
                .cloud_command_authorized("task", "command", "digest")
                .unwrap()
        );
    }
}
