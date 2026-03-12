use serde::{Deserialize, Serialize};

use crate::constants::{
    DEFAULT_VERIFY_CONTAINER_IMAGE, DEFAULT_VERIFY_CONTAINER_RUNTIME,
    DEFAULT_VERIFY_CONTAINER_WORKDIR, DEFAULT_VERIFY_MICROVM_MEMORY_MIB,
    DEFAULT_VERIFY_MICROVM_RUNTIME, DEFAULT_VERIFY_MICROVM_VCPU_COUNT,
    DEFAULT_VERIFY_MICROVM_WORKDIR,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyConfig {
    #[serde(default = "default_verify_commands")]
    pub commands: Vec<String>,
    #[serde(default)]
    pub mode: VerifyMode,
    #[serde(default)]
    pub container: VerifyContainerConfig,
    #[serde(default)]
    pub microvm: VerifyMicrovmConfig,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            commands: default_verify_commands(),
            mode: VerifyMode::Host,
            container: VerifyContainerConfig::default(),
            microvm: VerifyMicrovmConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    #[default]
    Host,
    Container,
    Microvm,
}

impl VerifyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Container => "container",
            Self::Microvm => "microvm",
        }
    }
}

impl std::str::FromStr for VerifyMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "host" => Ok(Self::Host),
            "container" => Ok(Self::Container),
            "microvm" => Ok(Self::Microvm),
            other => Err(format!(
                "invalid verify mode '{other}', expected one of: host, container, microvm"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyContainerConfig {
    #[serde(default = "default_verify_container_runtime")]
    pub runtime: String,
    #[serde(default = "default_verify_container_image")]
    pub image: String,
    #[serde(default = "default_verify_container_workdir")]
    pub workdir: String,
    #[serde(default)]
    pub fallback_to_host_on_unavailable: bool,
}

impl Default for VerifyContainerConfig {
    fn default() -> Self {
        Self {
            runtime: default_verify_container_runtime(),
            image: default_verify_container_image(),
            workdir: default_verify_container_workdir(),
            fallback_to_host_on_unavailable: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyMicrovmConfig {
    #[serde(default = "default_verify_microvm_runtime")]
    pub runtime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rootfs_image: Option<String>,
    #[serde(default = "default_verify_microvm_workdir")]
    pub workdir: String,
    #[serde(default = "default_verify_microvm_vcpu_count")]
    pub vcpu_count: u8,
    #[serde(default = "default_verify_microvm_memory_mib")]
    pub memory_mib: u32,
    #[serde(default)]
    pub fallback_to_container_on_unavailable: bool,
    #[serde(default)]
    pub fallback_to_host_on_unavailable: bool,
}

impl Default for VerifyMicrovmConfig {
    fn default() -> Self {
        Self {
            runtime: default_verify_microvm_runtime(),
            kernel_image: None,
            rootfs_image: None,
            workdir: default_verify_microvm_workdir(),
            vcpu_count: default_verify_microvm_vcpu_count(),
            memory_mib: default_verify_microvm_memory_mib(),
            fallback_to_container_on_unavailable: false,
            fallback_to_host_on_unavailable: false,
        }
    }
}

pub(crate) fn default_verify_commands() -> Vec<String> {
    vec![
        "cargo fmt --all --check".to_owned(),
        "cargo clippy --workspace -- -D warnings".to_owned(),
        "cargo test --workspace".to_owned(),
    ]
}

pub(crate) fn default_verify_container_runtime() -> String {
    DEFAULT_VERIFY_CONTAINER_RUNTIME.to_owned()
}

pub(crate) fn default_verify_container_image() -> String {
    DEFAULT_VERIFY_CONTAINER_IMAGE.to_owned()
}

pub(crate) fn default_verify_container_workdir() -> String {
    DEFAULT_VERIFY_CONTAINER_WORKDIR.to_owned()
}

pub(crate) fn default_verify_microvm_runtime() -> String {
    DEFAULT_VERIFY_MICROVM_RUNTIME.to_owned()
}

pub(crate) fn default_verify_microvm_workdir() -> String {
    DEFAULT_VERIFY_MICROVM_WORKDIR.to_owned()
}

pub(crate) fn default_verify_microvm_vcpu_count() -> u8 {
    DEFAULT_VERIFY_MICROVM_VCPU_COUNT
}

pub(crate) fn default_verify_microvm_memory_mib() -> u32 {
    DEFAULT_VERIFY_MICROVM_MEMORY_MIB
}
