//! Redacted durable effect admission. No tool payload, tokens or replay log.
use crate::config::Error;
use namzu_core::AgentError;
use namzu_session::{PrivateStore, StoreLock, now_ms};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
fn id() -> Result<String, Error> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|_| Error::Configuration)?;
    Ok(b.iter().map(|b| format!("{b:02x}")).collect())
}
fn valid(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    id: String,
    owner: String,
    target: String,
    server: String,
    method: String,
    arguments_sha256: String,
    started_ms: u64,
    uncertain: bool,
}
#[derive(Serialize)]
pub struct Report {
    #[serde(skip)]
    owner: String,
    pub id: String,
    pub target: String,
    pub server: String,
    pub method: String,
    pub arguments_sha256: String,
    pub started_ms: u64,
    pub active: bool,
    pub uncertain: bool,
}
pub(crate) struct Owner {
    id: String,
    _lease: StoreLock,
}
pub(crate) struct Ticket(String);
pub struct Store {
    records: PrivateStore,
    owners: PrivateStore,
}
impl Store {
    pub(crate) fn new(store: PrivateStore) -> Result<Self, Error> {
        Ok(Self {
            records: store.child("records")?,
            owners: store.child("owners")?,
        })
    }
    pub(crate) fn owner(&self) -> Result<Owner, Error> {
        let id = id()?;
        let lease = self.owners.lock(&id)?;
        Ok(Owner { id, _lease: lease })
    }
    fn read(&self, id: &str) -> Result<Option<Record>, Error> {
        if !valid(id, 32) {
            return Err(Error::Configuration);
        }
        let record: Option<Record> = self.records.read(&format!("{id}.json"))?;
        if let Some(r) = &record
            && (r.version != 1
                || r.id != id
                || !valid(&r.owner, 32)
                || !valid(&r.target, 64)
                || !valid(&r.arguments_sha256, 64)
                || !crate::config::valid_name(&r.server)
                || r.method != "tools/call"
                || r.started_ms == 0)
        {
            return Err(Error::Configuration);
        }
        Ok(record)
    }
    fn active(&self, owner: &str) -> Result<bool, Error> {
        match self.owners.lock(owner) {
            Ok(_) => Ok(false),
            Err(AgentError::State("state is busy in another process")) => Ok(true),
            Err(e) => Err(e.into()),
        }
    }
    fn reports(&self) -> Result<Vec<Report>, Error> {
        let mut reports = Vec::new();
        for name in self.records.names()? {
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            let r = self.read(id)?.ok_or(Error::Configuration)?;
            if reports.len() >= 256 {
                return Err(Error::Configuration);
            }
            let active = self.active(&r.owner)? && !r.uncertain;
            reports.push(Report {
                owner: r.owner,
                id: r.id,
                target: r.target,
                server: r.server,
                method: r.method,
                arguments_sha256: r.arguments_sha256,
                started_ms: r.started_ms,
                active,
                uncertain: r.uncertain || !active,
            });
        }
        Ok(reports)
    }
    pub fn list(&self) -> Result<Vec<Report>, Error> {
        let _lock = self.records.lock("records")?;
        self.reports()
    }
    pub(crate) fn check(&self, target: &str, owner: Option<&Owner>) -> Result<(), Error> {
        let _lock = self.records.lock("records")?;
        self.check_inner(target, owner)
    }
    fn check_inner(&self, target: &str, owner: Option<&Owner>) -> Result<(), Error> {
        for report in self.reports()? {
            if report.target == target
                && (report.uncertain || owner.is_some_and(|o| report.owner == o.id))
            {
                return Err(Error::Uncertain);
            }
        }
        Ok(())
    }
    pub(crate) fn begin(
        &self,
        owner: &Owner,
        target: String,
        server: String,
        params: &Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<Ticket, Error> {
        let _lock = self.records.lock("records")?;
        self.check_inner(&target, Some(owner))?;
        if self.reports()?.len() >= 256 {
            return Err(Error::Configuration);
        }
        let id = id()?;
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(params).map_err(|_| Error::Configuration)?)
        );
        self.records.write_new(
            &format!("{id}.json"),
            &Record {
                version: 1,
                id: id.clone(),
                owner: owner.id.clone(),
                target,
                server,
                method: "tools/call".into(),
                arguments_sha256: digest,
                started_ms: now_ms(),
                uncertain: false,
            },
        )?;
        Ok(Ticket(id))
    }
    pub(crate) fn finish(&self, ticket: Ticket, success: bool) -> Result<(), Error> {
        let _lock = self.records.lock("records")?;
        if success {
            self.records.remove(&format!("{}.json", ticket.0))?;
        } else {
            let mut r = self.read(&ticket.0)?.ok_or(Error::Configuration)?;
            r.uncertain = true;
            self.records.write(&format!("{}.json", ticket.0), &r)?;
        }
        Ok(())
    }
    /// Host confirms it inspected external effects. Does not replay/undo anything.
    pub fn acknowledge(&self, id: &str) -> Result<(), Error> {
        let _lock = self.records.lock("records")?;
        let r = self.read(id)?.ok_or(Error::NotFound)?;
        if !r.uncertain && self.active(&r.owner)? {
            return Err(Error::RecoveryActive);
        }
        self.records.remove(&format!("{id}.json"))?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn store(root: &std::path::Path) -> Store {
        Store::new(PrivateStore::open(root).unwrap()).unwrap()
    }
    #[test]
    fn live_owners_can_coexist_but_same_owner_cannot_overlap_an_effect() {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        let a = s.owner().unwrap();
        let b = s.owner().unwrap();
        let target = "a".repeat(64);
        let first = s
            .begin(&a, target.clone(), "fixture".into(), &None)
            .unwrap();
        assert!(s.check(&target, Some(&b)).is_ok());
        assert!(matches!(s.check(&target, Some(&a)), Err(Error::Uncertain)));
        let second = s
            .begin(&b, target.clone(), "fixture".into(), &None)
            .unwrap();
        assert_eq!(s.list().unwrap().len(), 2);
        s.finish(first, true).unwrap();
        s.finish(second, false).unwrap();
        assert!(matches!(s.check(&target, Some(&a)), Err(Error::Uncertain)));
    }
    #[test]
    fn abandoned_pending_owner_becomes_uncertain_and_acknowledgement_never_replays() {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        let owner = s.owner().unwrap();
        let _ticket = s
            .begin(&owner, "b".repeat(64), "fixture".into(), &None)
            .unwrap();
        let before = s.list().unwrap();
        assert!(before[0].active);
        assert!(matches!(
            s.acknowledge(&before[0].id),
            Err(Error::RecoveryActive)
        ));
        drop(owner);
        let after = s.list().unwrap();
        assert!(after[0].uncertain && !after[0].active);
        s.acknowledge(&after[0].id).unwrap();
        assert!(s.list().unwrap().is_empty());
    }
    #[test]
    fn corrupt_records_fail_closed_and_invalid_ack_ids_cannot_traverse() {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        s.records
            .write(
                &format!("{}.json", "a".repeat(32)),
                &serde_json::json!({"corrupt":true}),
            )
            .unwrap();
        assert!(s.list().is_err());
        assert!(s.check(&"a".repeat(64), None).is_err());
        assert!(s.acknowledge("../../profiles").is_err());
        assert_eq!(
            s.records
                .names()
                .unwrap()
                .iter()
                .filter(|s| s.ends_with(".json"))
                .count(),
            1
        );
    }
}
