//! Product-owned durable device command state. No camera addresses, passwords
//! or action payloads are persisted here. The command digest binds a stable ID
//! to its immutable request; configuration and pairing remain separate.
use crate::{CommandErrorCode, CommandResult, DeviceCommand};
use anyhow::{Context, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_RECORDS: usize = 2048;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum State {
    Queued,
    Pending,
    Completed { result: CommandResult },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    id: Uuid,
    camera_id: Uuid,
    command_sha256: String,
    dedup_until: DateTime<Utc>,
    acknowledged: bool,
    #[serde(rename = "execution")]
    state: State,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: u32,
    records: Vec<Record>,
}

pub(crate) struct CommandLedger {
    path: PathBuf,
    records: BTreeMap<Uuid, Record>,
}

type ReceiptMap = HashMap<Uuid, HashMap<Uuid, CommandResult>>;
type CompletedMap = HashMap<Uuid, (CommandResult, DateTime<Utc>)>;

fn fingerprint(command: &DeviceCommand) -> anyhow::Result<String> {
    Ok(Sha256::digest(serde_json::to_vec(command)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

impl CommandLedger {
    pub(crate) fn open(config: &Path) -> anyhow::Result<Self> {
        let name = config
            .file_name()
            .context("command_state_incompatible: config filename is missing")?;
        let mut ledger_name = name.to_os_string();
        ledger_name.push(".commands.json");
        let path = config.with_file_name(ledger_name);
        let records = match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => anyhow::bail!(
                "command_state_incompatible: command state cannot be inspected; it was preserved"
            ),
            Ok(metadata) => {
                ensure!(
                    metadata.is_file() && !metadata.file_type().is_symlink(),
                    "command_state_incompatible: command state must be a protected regular file; it was preserved"
                );
                #[cfg(windows)]
                if path.parent() == crate::default_config_path().parent() {
                    crate::windows_config_acl::secure_config_file(&path)?;
                }
                let bytes = crate::read_configuration_bytes(&path).context("command_state_incompatible: command state cannot be safely read; it was preserved")?;
                let document: Document = serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("command_state_incompatible: command state is corrupt or unsupported; it was preserved"))?;
                ensure!(
                    document.format == 1 && document.records.len() <= MAX_RECORDS,
                    "command_state_incompatible: command state format or size is unsupported; it was preserved"
                );
                document.records
            }
        };
        let mut ledger = Self {
            path,
            records: BTreeMap::new(),
        };
        let mut recovered = false;
        for mut record in records {
            ensure!(
                record.command_sha256.len() == 64
                    && record
                        .command_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "command_state_incompatible: invalid command digest; state was preserved"
            );
            match &record.state {
                State::Queued => {
                    ensure!(
                        !record.acknowledged,
                        "command_state_incompatible: invalid queued state"
                    );
                    record.state = State::Completed {
                        result: CommandResult::failed(
                            record.id,
                            CommandErrorCode::DeviceUnavailable,
                        ),
                    };
                    recovered = true;
                }
                State::Pending => {
                    ensure!(
                        !record.acknowledged,
                        "command_state_incompatible: invalid pending state"
                    );
                    record.state = State::Completed {
                        result: CommandResult::unknown(record.id),
                    };
                    recovered = true;
                }
                State::Completed { result } => {
                    ensure!(
                        result.id == record.id,
                        "command_state_incompatible: command result identity differs"
                    );
                    result
                        .validate()
                        .context("command_state_incompatible: invalid command outcome")?;
                }
            }
            ensure!(
                ledger.records.insert(record.id, record).is_none(),
                "command_state_incompatible: duplicate command identity"
            );
        }
        recovered |= ledger.prune();
        if recovered {
            ledger.persist()?;
        }
        Ok(ledger)
    }

    fn prune(&mut self) -> bool {
        let before = self.records.len();
        let now = Utc::now();
        self.records
            .retain(|_, record| !record.acknowledged || record.dedup_until > now);
        before != self.records.len()
    }

    fn persist(&self) -> anyhow::Result<()> {
        let document = Document {
            format: 1,
            records: self.records.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec(&document)?;
        crate::save_configuration_bytes(&self.path, &bytes).context("command_state_incompatible: durable command state could not be saved; physical actions were stopped")
    }

    pub(crate) fn remaining(&self) -> usize {
        MAX_RECORDS.saturating_sub(self.records.len())
    }

    pub(crate) fn receipts(&self) -> (ReceiptMap, CompletedMap) {
        let mut pending = HashMap::new();
        let mut completed = HashMap::new();
        for record in self.records.values() {
            if let State::Completed { result } = &record.state {
                if !record.acknowledged {
                    pending
                        .entry(record.camera_id)
                        .or_insert_with(HashMap::new)
                        .insert(record.id, result.clone());
                }
                completed.insert(record.id, (result.clone(), record.dedup_until));
            }
        }
        (pending, completed)
    }

    pub(crate) fn admit(&mut self, commands: &[DeviceCommand]) -> anyhow::Result<()> {
        self.prune();
        let mut additions: BTreeMap<Uuid, Record> = BTreeMap::new();
        for command in commands {
            let digest = fingerprint(command)?;
            if let Some(record) = self
                .records
                .get(&command.id)
                .or_else(|| additions.get(&command.id))
            {
                ensure!(
                    record.camera_id == command.camera_id && record.command_sha256 == digest,
                    "command_state_incompatible: command ID was reused with a different request"
                );
            } else {
                let expires_at = DateTime::parse_from_rfc3339(&command.expires_at)
                    .map(|time| time.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                additions.insert(
                    command.id,
                    Record {
                        id: command.id,
                        camera_id: command.camera_id,
                        command_sha256: digest,
                        dedup_until: expires_at
                            .checked_add_signed(chrono::Duration::seconds(120))
                            .unwrap_or(expires_at),
                        acknowledged: false,
                        state: State::Queued,
                    },
                );
            }
        }
        self.prune();
        if self.records.len() + additions.len() > MAX_RECORDS {
            return Err(crate::CommandBackpressure.into());
        }
        if !additions.is_empty() {
            self.records.extend(additions);
            self.persist()?;
        }
        Ok(())
    }

    pub(crate) fn replay(&self, command: &DeviceCommand) -> anyhow::Result<Option<CommandResult>> {
        let record = self
            .records
            .get(&command.id)
            .context("command_state_incompatible: command was not durably admitted")?;
        ensure!(
            record.camera_id == command.camera_id && record.command_sha256 == fingerprint(command)?,
            "command_state_incompatible: command identity changed"
        );
        Ok(match &record.state {
            State::Queued => None,
            State::Pending => Some(CommandResult::unknown(command.id)),
            State::Completed { result } => Some(result.clone()),
        })
    }

    pub(crate) fn begin(&mut self, id: Uuid) -> anyhow::Result<()> {
        let record = self
            .records
            .get_mut(&id)
            .context("command_state_incompatible: command intent is missing")?;
        ensure!(
            matches!(record.state, State::Queued),
            "command_state_incompatible: command is already pending or terminal"
        );
        record.state = State::Pending;
        self.persist()
    }

    pub(crate) fn complete(&mut self, result: &CommandResult) -> anyhow::Result<()> {
        result.validate()?;
        let record = self
            .records
            .get_mut(&result.id)
            .context("command_state_incompatible: command intent is missing")?;
        record.state = State::Completed {
            result: result.clone(),
        };
        self.persist()
    }

    pub(crate) fn cancel(&mut self, id: Uuid) -> anyhow::Result<CommandResult> {
        let record = self
            .records
            .get(&id)
            .context("command_state_incompatible: canceled command intent is missing")?;
        let result = match &record.state {
            State::Queued => CommandResult::failed(id, CommandErrorCode::DeviceUnavailable),
            State::Pending => CommandResult::unknown(id),
            State::Completed { result } => result.clone(),
        };
        self.complete(&result)?;
        Ok(result)
    }

    pub(crate) fn acknowledge(&mut self, camera: Uuid, ids: &[Uuid]) -> anyhow::Result<()> {
        let mut changed = false;
        for id in ids {
            if let Some(record) = self.records.get_mut(id) {
                ensure!(
                    record.camera_id == camera && matches!(record.state, State::Completed { .. }),
                    "command_state_incompatible: acknowledgement does not match a terminal command"
                );
                changed |= !record.acknowledged;
                record.acknowledged = true;
            }
        }
        changed |= self.prune();
        if changed {
            self.persist()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command() -> DeviceCommand {
        DeviceCommand {
            id: Uuid::new_v4(),
            camera_id: Uuid::new_v4(),
            kind: "ptz".into(),
            payload: serde_json::json!({"action":"move","pan":0.5}),
            expires_at: (Utc::now() + chrono::Duration::seconds(30)).to_rfc3339(),
        }
    }
    #[test]
    fn interrupted_physical_action_becomes_unknown_after_restart_without_replay() {
        let root = crate::tests::tempdir();
        let config = root.path().join("config.json");
        let command = command();
        let mut ledger = CommandLedger::open(&config).unwrap();
        ledger.admit(std::slice::from_ref(&command)).unwrap();
        ledger.begin(command.id).unwrap();
        drop(ledger);
        let ledger = CommandLedger::open(&config).unwrap();
        let result = ledger.replay(&command).unwrap().unwrap();
        assert_eq!(result, CommandResult::unknown(command.id));
        assert_eq!(ledger.receipts().0[&command.camera_id][&command.id], result);
        let mut changed = command.clone();
        changed.payload = serde_json::json!({"action":"stop"});
        assert!(ledger.replay(&changed).is_err());
    }
    #[test]
    fn queued_interruption_is_confirmed_unavailable_and_ack_preserves_deduplication() {
        let root = crate::tests::tempdir();
        let config = root.path().join("config.json");
        let command = command();
        let mut ledger = CommandLedger::open(&config).unwrap();
        ledger.admit(std::slice::from_ref(&command)).unwrap();
        drop(ledger);
        let mut ledger = CommandLedger::open(&config).unwrap();
        assert_eq!(
            ledger.replay(&command).unwrap().unwrap().error_code,
            Some(CommandErrorCode::DeviceUnavailable)
        );
        ledger
            .acknowledge(command.camera_id, &[command.id])
            .unwrap();
        drop(ledger);
        let ledger = CommandLedger::open(&config).unwrap();
        assert!(ledger.receipts().0.is_empty());
        assert!(ledger.replay(&command).unwrap().is_some());
    }
    #[test]
    fn complete_durable_budget_fits_byte_limit_and_remains_bounded_after_recovery() {
        let root = crate::tests::tempdir();
        let config = root.path().join("config.json");
        let commands = (0..MAX_RECORDS).map(|_| command()).collect::<Vec<_>>();
        let mut ledger = CommandLedger::open(&config).unwrap();
        ledger.admit(&commands).unwrap();
        assert_eq!(ledger.remaining(), 0);
        let original = std::fs::read(&ledger.path).unwrap();
        assert!(ledger.admit(&[command()]).is_err());
        assert_eq!(std::fs::read(&ledger.path).unwrap(), original);
        drop(ledger);
        let recovered = CommandLedger::open(&config).unwrap();
        assert_eq!(recovered.records.len(), MAX_RECORDS);
        assert_eq!(recovered.remaining(), 0);
        assert!(std::fs::metadata(&recovered.path).unwrap().len() <= crate::MAX_INPUT_BYTES);
        assert_eq!(
            recovered
                .receipts()
                .0
                .values()
                .map(HashMap::len)
                .sum::<usize>(),
            MAX_RECORDS
        );
    }

    #[test]
    fn corruption_and_persistence_failure_preserve_state_and_prevent_physical_dispatch() {
        let root = crate::tests::tempdir();
        let config = root.path().join("config.json");
        let command = command();
        let mut ledger = CommandLedger::open(&config).unwrap();
        ledger.admit(std::slice::from_ref(&command)).unwrap();
        let path = ledger.path.clone();
        std::fs::write(&path, b"{broken credential-looking-value}").unwrap();
        assert!(CommandLedger::open(&config).is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"{broken credential-looking-value}"
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(ledger.begin(command.id).is_err());
    }
}
