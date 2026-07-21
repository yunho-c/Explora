use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use glob::{glob, Pattern};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::filesystem::ExplorerError;

const TARGETS_FILE_VERSION: u32 = 1;
const MAX_INCLUDE_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SshTargetSource {
    Manual,
    OpenSshConfig,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SshTargetSummaryDto {
    pub id: String,
    pub location_id: String,
    pub name: String,
    pub source: SshTargetSource,
    pub endpoint: String,
    pub status: &'static str,
    pub editable: bool,
    pub connected_location_id: Option<String>,
    pub configuration: Option<ManualSshTargetInputDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualSshTargetInputDto {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub initial_path: Option<String>,
    pub identity_file: Option<String>,
    pub identities_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredTarget {
    id: String,
    name: String,
    host: String,
    port: u16,
    username: String,
    initial_path: Option<String>,
    identity_file: Option<String>,
    identities_only: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredTargetsDocument {
    version: u32,
    targets: Vec<StoredTarget>,
}

#[derive(Debug, Clone)]
pub struct ResolvedSshTarget {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub initial_path: String,
    pub identity_files: Vec<PathBuf>,
    pub identities_only: bool,
    pub known_hosts_path: PathBuf,
}

pub struct SshTargetStore {
    storage_path: PathBuf,
    home_dir: PathBuf,
    manual_targets: Mutex<Vec<StoredTarget>>,
}

impl SshTargetStore {
    pub fn new(storage_path: PathBuf, home_dir: PathBuf) -> Result<Self, ExplorerError> {
        let manual_targets = load_targets(&storage_path)?;
        Ok(Self {
            storage_path,
            home_dir,
            manual_targets: Mutex::new(manual_targets),
        })
    }

    pub fn list(&self) -> Result<Vec<SshTargetSummaryDto>, ExplorerError> {
        let manual = self
            .manual_targets
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?
            .clone();
        let mut summaries = manual
            .into_iter()
            .map(|target| summary_for_stored(&target))
            .collect::<Vec<_>>();

        let config = load_open_ssh_config(&self.home_dir)?;
        summaries.extend(config.aliases.into_iter().map(|alias| {
            let resolved = resolve_config_alias(&config.contents, &self.home_dir, &alias);
            let (id, endpoint) = match resolved {
                Ok(target) => (
                    target.id,
                    endpoint(&target.username, &target.host, target.port),
                ),
                Err(_) => (format!("config:{alias}"), "SSH config".to_owned()),
            };
            SshTargetSummaryDto {
                location_id: location_id(&id),
                id,
                name: alias,
                source: SshTargetSource::OpenSshConfig,
                endpoint,
                status: "disconnected",
                editable: false,
                connected_location_id: None,
                configuration: None,
            }
        }));
        Ok(summaries)
    }

    pub fn create(
        &self,
        input: ManualSshTargetInputDto,
    ) -> Result<SshTargetSummaryDto, ExplorerError> {
        validate_input(&input)?;
        let target = StoredTarget {
            id: format!("manual:{}", Uuid::new_v4()),
            name: input.name.trim().to_owned(),
            host: input.host.trim().to_owned(),
            port: input.port,
            username: input.username.trim().to_owned(),
            initial_path: normalized_optional(input.initial_path),
            identity_file: normalized_optional(input.identity_file),
            identities_only: input.identities_only,
        };
        let summary = summary_for_stored(&target);
        let mut targets = self
            .manual_targets
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let mut updated = targets.clone();
        updated.push(target);
        persist_targets(&self.storage_path, &updated)?;
        *targets = updated;
        Ok(summary)
    }

    pub fn update(
        &self,
        id: &str,
        input: ManualSshTargetInputDto,
    ) -> Result<SshTargetSummaryDto, ExplorerError> {
        validate_input(&input)?;
        let mut targets = self
            .manual_targets
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let mut updated = targets.clone();
        let target = updated
            .iter_mut()
            .find(|target| target.id == id)
            .ok_or(ExplorerError::InvalidReference)?;
        target.name = input.name.trim().to_owned();
        target.host = input.host.trim().to_owned();
        target.port = input.port;
        target.username = input.username.trim().to_owned();
        target.initial_path = normalized_optional(input.initial_path);
        target.identity_file = normalized_optional(input.identity_file);
        target.identities_only = input.identities_only;
        let summary = summary_for_stored(target);
        persist_targets(&self.storage_path, &updated)?;
        *targets = updated;
        Ok(summary)
    }

    pub fn delete(&self, id: &str) -> Result<(), ExplorerError> {
        let mut targets = self
            .manual_targets
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let mut updated = targets.clone();
        let original_len = updated.len();
        updated.retain(|target| target.id != id);
        if updated.len() == original_len {
            return Err(ExplorerError::InvalidReference);
        }
        persist_targets(&self.storage_path, &updated)?;
        *targets = updated;
        Ok(())
    }

    pub fn resolve(&self, id: &str) -> Result<ResolvedSshTarget, ExplorerError> {
        if let Some(alias) = id.strip_prefix("config:") {
            let config = load_open_ssh_config(&self.home_dir)?;
            if !config.aliases.iter().any(|candidate| candidate == alias) {
                return Err(ExplorerError::InvalidReference);
            }
            return resolve_config_alias(&config.contents, &self.home_dir, alias);
        }

        let targets = self
            .manual_targets
            .lock()
            .map_err(|_| ExplorerError::StateUnavailable)?;
        let target = targets
            .iter()
            .find(|target| target.id == id)
            .ok_or(ExplorerError::InvalidReference)?;
        Ok(resolve_stored(target, &self.home_dir))
    }
}

fn validate_input(input: &ManualSshTargetInputDto) -> Result<(), ExplorerError> {
    for (label, value, max) in [
        ("name", input.name.as_str(), 80),
        ("host", input.host.as_str(), 255),
        ("username", input.username.as_str(), 128),
    ] {
        let value = value.trim();
        if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
            return Err(ExplorerError::InvalidConfiguration(format!(
                "The SSH target {label} is invalid."
            )));
        }
    }
    for (label, value) in [
        ("initial path", input.initial_path.as_deref()),
        ("identity file", input.identity_file.as_deref()),
    ] {
        if let Some(value) = value {
            if value.len() > 4096 || value.chars().any(char::is_control) {
                return Err(ExplorerError::InvalidConfiguration(format!(
                    "The SSH target {label} is invalid."
                )));
            }
        }
    }
    if input.port == 0 {
        return Err(ExplorerError::InvalidConfiguration(
            "The SSH target port must be between 1 and 65535.".to_owned(),
        ));
    }
    Ok(())
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn summary_for_stored(target: &StoredTarget) -> SshTargetSummaryDto {
    SshTargetSummaryDto {
        id: target.id.clone(),
        location_id: location_id(&target.id),
        name: target.name.clone(),
        source: SshTargetSource::Manual,
        endpoint: endpoint(&target.username, &target.host, target.port),
        status: "disconnected",
        editable: true,
        connected_location_id: None,
        configuration: Some(ManualSshTargetInputDto {
            name: target.name.clone(),
            host: target.host.clone(),
            port: target.port,
            username: target.username.clone(),
            initial_path: target.initial_path.clone(),
            identity_file: target.identity_file.clone(),
            identities_only: target.identities_only,
        }),
    }
}

pub fn location_id(target_id: &str) -> String {
    format!("ssh:{target_id}")
}

fn endpoint(username: &str, host: &str, port: u16) -> String {
    if port == 22 {
        format!("{username}@{host}")
    } else {
        format!("{username}@{host}:{port}")
    }
}

fn resolve_stored(target: &StoredTarget, home_dir: &Path) -> ResolvedSshTarget {
    let identity_files = target
        .identity_file
        .as_deref()
        .map(|path| vec![expand_home(path, home_dir)])
        .unwrap_or_else(|| default_identity_files(home_dir));
    ResolvedSshTarget {
        id: target.id.clone(),
        name: target.name.clone(),
        host: target.host.clone(),
        port: target.port,
        username: target.username.clone(),
        initial_path: target
            .initial_path
            .clone()
            .unwrap_or_else(|| ".".to_owned()),
        identity_files,
        identities_only: target.identities_only,
        known_hosts_path: home_dir.join(".ssh").join("known_hosts"),
    }
}

struct OpenSshConfig {
    contents: String,
    aliases: Vec<String>,
}

fn load_open_ssh_config(home_dir: &Path) -> Result<OpenSshConfig, ExplorerError> {
    let path = home_dir.join(".ssh").join("config");
    if !path.is_file() {
        return Ok(OpenSshConfig {
            contents: String::new(),
            aliases: Vec::new(),
        });
    }
    let mut visited = HashSet::new();
    let contents = load_config_file(&path, home_dir, 0, &mut visited)?;
    let mut aliases = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        let Some((key, values)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if !key.eq_ignore_ascii_case("host") {
            continue;
        }
        for alias in values.split_ascii_whitespace() {
            if !alias.starts_with('!')
                && !alias.contains(['*', '?'])
                && !aliases.iter().any(|existing| existing == alias)
            {
                aliases.push(alias.to_owned());
            }
        }
    }
    Ok(OpenSshConfig { contents, aliases })
}

fn load_config_file(
    path: &Path,
    home_dir: &Path,
    depth: usize,
    visited: &mut HashSet<PathBuf>,
) -> Result<String, ExplorerError> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(ExplorerError::InvalidConfiguration(
            "OpenSSH config includes are nested too deeply.".to_owned(),
        ));
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canonical) {
        return Ok(String::new());
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| ExplorerError::io("read OpenSSH config", path, error))?;
    let mut expanded = String::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        let include = trimmed
            .split_once(char::is_whitespace)
            .and_then(|(key, value)| key.eq_ignore_ascii_case("include").then_some(value.trim()));
        if let Some(patterns) = include {
            for pattern in patterns.split_ascii_whitespace() {
                let pattern = expand_home(pattern.trim_matches(['\'', '"']), home_dir);
                let Some(pattern) = pattern.to_str() else {
                    continue;
                };
                let matches = glob(pattern).map_err(|_| {
                    ExplorerError::InvalidConfiguration(
                        "An OpenSSH Include pattern is invalid.".to_owned(),
                    )
                })?;
                for included in matches.flatten() {
                    expanded.push_str(&load_config_file(&included, home_dir, depth + 1, visited)?);
                    expanded.push('\n');
                }
            }
        } else {
            expanded.push_str(line);
            expanded.push('\n');
        }
    }
    Ok(expanded)
}

fn resolve_config_alias(
    contents: &str,
    home_dir: &Path,
    alias: &str,
) -> Result<ResolvedSshTarget, ExplorerError> {
    let config = russh_config::parse(contents, alias).map_err(|error| {
        ExplorerError::InvalidConfiguration(format!(
            "Explora could not resolve SSH alias {alias}: {error}"
        ))
    })?;
    if config.host_config.proxy_command.is_some() {
        return Err(ExplorerError::Unsupported(format!(
            "SSH alias {alias} uses ProxyCommand, which Explora does not execute."
        )));
    }
    if config.host_config.proxy_jump.is_some() {
        return Err(ExplorerError::Unsupported(format!(
            "SSH alias {alias} uses ProxyJump, which Explora does not support yet."
        )));
    }
    let identity_files = config
        .host_config
        .identity_file
        .clone()
        .filter(|files| !files.is_empty())
        .unwrap_or_else(|| default_identity_files(home_dir));
    let known_hosts_path = config
        .host_config
        .user_known_hosts_file
        .clone()
        .unwrap_or_else(|| home_dir.join(".ssh").join("known_hosts"));
    Ok(ResolvedSshTarget {
        id: format!("config:{alias}"),
        name: alias.to_owned(),
        host: config.host().to_owned(),
        port: config.port(),
        username: config.user(),
        initial_path: ".".to_owned(),
        identity_files,
        identities_only: resolve_identities_only(contents, alias),
        known_hosts_path,
    })
}

fn resolve_identities_only(contents: &str, alias: &str) -> bool {
    let mut block_matches = true;
    let mut resolved = None;
    for line in contents.lines() {
        let line = line.trim();
        let Some((key, values)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if key.eq_ignore_ascii_case("host") {
            block_matches = values.split_ascii_whitespace().any(|pattern| {
                if pattern.starts_with('!') {
                    return false;
                }
                Pattern::new(pattern)
                    .map(|pattern| pattern.matches(alias))
                    .unwrap_or(false)
            });
        } else if block_matches && resolved.is_none() && key.eq_ignore_ascii_case("identitiesonly")
        {
            resolved = Some(values.trim().eq_ignore_ascii_case("yes"));
        }
    }
    resolved.unwrap_or(false)
}

fn default_identity_files(home_dir: &Path) -> Vec<PathBuf> {
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .into_iter()
        .map(|name| home_dir.join(".ssh").join(name))
        .filter(|path| path.is_file())
        .collect()
}

fn expand_home(path: &str, home_dir: &Path) -> PathBuf {
    path.strip_prefix("~/")
        .map(|suffix| home_dir.join(suffix))
        .unwrap_or_else(|| PathBuf::from(path))
}

fn load_targets(path: &Path) -> Result<Vec<StoredTarget>, ExplorerError> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let bytes =
        fs::read(path).map_err(|error| ExplorerError::io("read SSH targets", path, error))?;
    let document: StoredTargetsDocument = serde_json::from_slice(&bytes).map_err(|_| {
        ExplorerError::InvalidConfiguration(
            "Explora's saved SSH target file is malformed.".to_owned(),
        )
    })?;
    if document.version != TARGETS_FILE_VERSION {
        return Err(ExplorerError::InvalidConfiguration(
            "Explora's saved SSH target file has an unsupported version.".to_owned(),
        ));
    }
    Ok(document.targets)
}

fn persist_targets(path: &Path, targets: &[StoredTarget]) -> Result<(), ExplorerError> {
    let parent = path.parent().ok_or_else(|| {
        ExplorerError::InvalidConfiguration("The SSH target storage path is invalid.".to_owned())
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| ExplorerError::io("create its configuration directory", parent, error))?;
    let document = StoredTargetsDocument {
        version: TARGETS_FILE_VERSION,
        targets: targets.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&document).map_err(|_| {
        ExplorerError::Unexpected("Explora could not encode its SSH targets.".to_owned())
    })?;
    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|error| ExplorerError::io("create temporary SSH target storage", parent, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| ExplorerError::io("secure SSH target storage", path, error))?;
    }
    temporary
        .write_all(&bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|error| ExplorerError::io("write SSH targets", path, error))?;
    temporary
        .persist(path)
        .map_err(|error| ExplorerError::io("replace SSH target storage", path, error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn input() -> ManualSshTargetInputDto {
        ManualSshTargetInputDto {
            name: "Staging".to_owned(),
            host: "staging.example.com".to_owned(),
            port: 22,
            username: "deploy".to_owned(),
            initial_path: Some("/srv/app".to_owned()),
            identity_file: None,
            identities_only: false,
        }
    }

    #[test]
    fn persists_only_non_secret_target_metadata() {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("config/ssh-targets.json");
        let store = SshTargetStore::new(path.clone(), temp.path().to_path_buf()).expect("store");
        let summary = store.create(input()).expect("target");

        assert_eq!(summary.name, "Staging");
        let contents = fs::read_to_string(path).expect("persisted targets");
        assert!(contents.contains("staging.example.com"));
        assert!(!contents.to_lowercase().contains("password"));
    }

    #[test]
    fn discovers_only_concrete_config_aliases_and_rejects_proxies() {
        let temp = TempDir::new().expect("temporary directory");
        let ssh = temp.path().join(".ssh");
        fs::create_dir_all(&ssh).expect("ssh directory");
        fs::write(
            ssh.join("config"),
            "Host *\n  User yunho\nHost staging *.internal !blocked\n  HostName 10.0.0.5\nHost jump-only\n  ProxyJump bastion\n",
        )
        .expect("ssh config");
        let store = SshTargetStore::new(
            temp.path().join("app/ssh-targets.json"),
            temp.path().to_path_buf(),
        )
        .expect("store");

        let targets = store.list().expect("targets");
        assert!(targets.iter().any(|target| target.name == "staging"));
        assert!(!targets.iter().any(|target| target.name == "*.internal"));
        assert!(matches!(
            store.resolve("config:jump-only"),
            Err(ExplorerError::Unsupported(_))
        ));
    }
}
