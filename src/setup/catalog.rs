use super::{atomic_write_private, connector_home, now_epoch_seconds, source};
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MANIFEST_SCHEMA_VERSION: u32 = 4;
const MANIFEST_KIND: &str = "baijimu.codex.customer-install-artifacts";
const CACHE_SCHEMA_VERSION: u32 = 1;
const PAGINATED_THREADS_MINIMUM_VERSION: &str = "0.149.0";
const CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const CATALOG_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug)]
pub(crate) struct CliRequirement {
    target: Version,
    snapshot_id: Option<String>,
    source: &'static str,
    warning: Option<String>,
}

impl CliRequirement {
    pub(crate) fn target(&self) -> &Version {
        &self.target
    }

    pub(crate) fn status_value(&self) -> Value {
        json!({
            "requiredVersion": self.target.to_string(),
            "snapshotId": self.snapshot_id,
            "source": self.source,
            "warning": self.warning,
        })
    }
}

#[derive(Clone, Default)]
pub(super) struct CatalogResolver {
    state: Arc<Mutex<Option<ResolvedCatalog>>>,
}

impl CatalogResolver {
    pub(super) fn requirement(&self) -> CliRequirement {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(current) = state.as_ref() {
            if current.checked_at.elapsed() < current.refresh_after {
                return current.requirement.clone();
            }
        }

        let resolved = match fetch_current_catalog() {
            Ok(requirement) => {
                let _ = persist_catalog(&requirement);
                ResolvedCatalog {
                    requirement,
                    checked_at: Instant::now(),
                    refresh_after: CATALOG_REFRESH_INTERVAL,
                }
            }
            Err(error) => {
                let warning = format!("读取当前 Codex CLI 制品目录失败：{error:#}");
                let requirement = load_persisted_catalog()
                    .map(|mut requirement| {
                        requirement.source = "verified_cache";
                        requirement.warning = Some(warning.clone());
                        requirement
                    })
                    .unwrap_or_else(|| protocol_floor(Some(warning)));
                ResolvedCatalog {
                    requirement,
                    checked_at: Instant::now(),
                    refresh_after: CATALOG_RETRY_INTERVAL,
                }
            }
        };
        let requirement = resolved.requirement.clone();
        *state = Some(resolved);
        requirement
    }
}

#[derive(Clone)]
struct ResolvedCatalog {
    requirement: CliRequirement,
    checked_at: Instant,
    refresh_after: Duration,
}

#[derive(Debug, Deserialize)]
struct ManifestIdentity {
    schema_version: u32,
    manifest_kind: String,
    snapshot_id: String,
    components: ManifestComponents,
}

#[derive(Debug, Deserialize)]
struct ManifestComponents {
    codex_cli: CodexCliIdentity,
}

#[derive(Debug, Deserialize)]
struct CodexCliIdentity {
    tag_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedCatalog {
    schema_version: u32,
    snapshot_id: String,
    target_version: String,
    checked_at_epoch_seconds: u64,
}

fn fetch_current_catalog() -> Result<CliRequirement> {
    let url = source::manifest_url()?;
    let response = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .context("创建 Codex CLI 制品目录客户端失败")?
        .get(&url)
        .send()
        .with_context(|| format!("下载 Codex CLI 制品目录失败：{url}"))?
        .error_for_status()
        .with_context(|| format!("Codex CLI 制品目录返回失败状态：{url}"))?;
    let bytes = response.bytes().context("读取 Codex CLI 制品目录失败")?;
    requirement_from_manifest(&bytes)
}

fn requirement_from_manifest(bytes: &[u8]) -> Result<CliRequirement> {
    let manifest = crate::json_compat::from_slice::<ManifestIdentity>(bytes)
        .context("解析 Codex CLI 制品目录失败")?;
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        anyhow::bail!(
            "不支持的 Codex CLI 制品目录版本：{}",
            manifest.schema_version
        );
    }
    if manifest.manifest_kind != MANIFEST_KIND || manifest.snapshot_id.trim().is_empty() {
        anyhow::bail!("Codex CLI 制品目录身份无效");
    }
    let published = parse_release_tag(&manifest.components.codex_cli.tag_name)?;
    let minimum = protocol_minimum();
    let target = published.max(minimum);
    Ok(CliRequirement {
        target,
        snapshot_id: Some(manifest.snapshot_id),
        source: "artifact_catalog",
        warning: None,
    })
}

fn persist_catalog(requirement: &CliRequirement) -> Result<()> {
    let Some(snapshot_id) = requirement.snapshot_id.as_ref() else {
        return Ok(());
    };
    let cache = PersistedCatalog {
        schema_version: CACHE_SCHEMA_VERSION,
        snapshot_id: snapshot_id.clone(),
        target_version: requirement.target.to_string(),
        checked_at_epoch_seconds: now_epoch_seconds(),
    };
    atomic_write_private(&cache_path(), &serde_json::to_vec_pretty(&cache)?)
}

fn load_persisted_catalog() -> Option<CliRequirement> {
    let bytes = fs::read(cache_path()).ok()?;
    let cache = crate::json_compat::from_slice::<PersistedCatalog>(&bytes).ok()?;
    if cache.schema_version != CACHE_SCHEMA_VERSION || cache.snapshot_id.trim().is_empty() {
        return None;
    }
    let cached = Version::parse(&cache.target_version).ok()?;
    Some(CliRequirement {
        target: cached.max(protocol_minimum()),
        snapshot_id: Some(cache.snapshot_id),
        source: "verified_cache",
        warning: None,
    })
}

fn protocol_floor(warning: Option<String>) -> CliRequirement {
    CliRequirement {
        target: protocol_minimum(),
        snapshot_id: None,
        source: "protocol_floor",
        warning,
    }
}

fn protocol_minimum() -> Version {
    Version::parse(PAGINATED_THREADS_MINIMUM_VERSION)
        .expect("paginated threads minimum version must be valid semver")
}

fn parse_release_tag(tag: &str) -> Result<Version> {
    let version = tag
        .trim()
        .strip_prefix("rust-v")
        .or_else(|| tag.trim().strip_prefix('v'))
        .unwrap_or_else(|| tag.trim());
    Version::parse(version).with_context(|| format!("Codex CLI 发布标签不是有效 SemVer：{tag}"))
}

fn cache_path() -> std::path::PathBuf {
    connector_home()
        .join("setup")
        .join("cli-catalog-cache.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_release_tags_as_semver() {
        assert_eq!(
            parse_release_tag("rust-v0.149.0").unwrap(),
            Version::new(0, 149, 0)
        );
        assert_eq!(
            parse_release_tag("rust-v0.150.0-alpha.1").unwrap(),
            Version::parse("0.150.0-alpha.1").unwrap()
        );
        assert!(parse_release_tag("nightly").is_err());
    }

    #[test]
    fn protocol_floor_covers_paginated_thread_history() {
        assert_eq!(protocol_minimum(), Version::new(0, 149, 0));
    }

    #[test]
    fn validated_catalog_cannot_lower_the_protocol_floor() {
        let requirement = requirement_from_manifest(include_bytes!(
            "../../test/fixtures/codex-artifacts-manifest-v4.json"
        ))
        .unwrap();

        assert_eq!(requirement.target(), &Version::new(0, 149, 0));
        assert_eq!(requirement.snapshot_id.as_deref(), Some("fixture-snapshot"));
        assert_eq!(requirement.source, "artifact_catalog");
    }
}
