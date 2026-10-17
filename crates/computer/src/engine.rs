use crate::{
    ComputerRecord, ComputerSpec, labels,
    protocol::{GUEST, GuestReply, GuestRequest, WIRE_LIMIT},
};
use namzu_core::{AgentError, TurnContext};
use namzu_tools::process::{ProcessSpec, run_process_input};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mount {
    #[serde(rename = "Type")]
    pub kind: String,
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "Destination")]
    pub destination: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub running: bool,
    pub labels: BTreeMap<String, String>,
    pub memory: u64,
    pub swap: i64,
    pub cpus: u64,
    pub pids: i64,
    pub readonly: bool,
    pub privileged: bool,
    pub user: String,
    pub network: String,
    pub caps: Vec<String>,
    pub security: Vec<String>,
    pub mounts: Vec<Mount>,
    pub binds: Vec<String>,
    pub devices: Vec<serde_json::Value>,
    pub restart: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub name: String,
    pub labels: BTreeMap<String, String>,
}
pub enum Operation {
    Image(String),
    Inspect(String),
    VolumeInspect(String),
    CreateVolume {
        record: ComputerRecord,
    },
    Create {
        record: ComputerRecord,
    },
    Start(String),
    Kill(String),
    Remove(String),
    RemoveVolume(String),
    Exec {
        container: String,
        request: GuestRequest,
    },
}
pub enum Answer {
    Image(bool),
    Container(Option<Box<ContainerInfo>>),
    Volume(Option<VolumeInfo>),
    Done,
    Guest(GuestReply),
}
/// Trusted daemon adapter; fixtures implement this without Docker/model accounts.
pub trait Engine {
    fn perform(
        &mut self,
        operation: Operation,
        context: &TurnContext,
    ) -> Result<Answer, AgentError>;
}

pub struct Docker {
    program: PathBuf,
}
impl Docker {
    pub fn local(program: impl Into<PathBuf>) -> Result<Self, AgentError> {
        let program = program.into();
        if !program.is_absolute() {
            return Err(AgentError::Configuration(
                "absolute trusted Docker executable required",
            ));
        }
        Ok(Self { program })
    }
    fn command(
        &self,
        args: Vec<String>,
        input: &[u8],
        context: &TurnContext,
    ) -> Result<String, AgentError> {
        let mut env = BTreeMap::new();
        // Keep managed registry/proxy/CA selectors, never model credentials or endpoint overrides.
        for key in [
            "PATH",
            "HOME",
            "DOCKER_CONFIG",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "NO_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "no_proxy",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
            "REQUESTS_CA_BUNDLE",
        ] {
            if let Ok(value) = std::env::var(key) {
                env.insert(key.into(), value);
            }
        }
        let mut argv = vec!["--host=unix:///var/run/docker.sock".into()];
        argv.extend(args);
        let out = run_process_input(
            ProcessSpec {
                program: self.program.clone(),
                args: argv,
                cwd: "/".into(),
                env,
                timeout: context.remaining()?.min(Duration::from_secs(300)),
                output_limit: WIRE_LIMIT,
            },
            context,
            input,
        )?;
        if out.status != "exited" || out.exit_code != Some(0) {
            return Err(AgentError::State(
                "Docker operation failed; no host fallback",
            ));
        }
        Ok(out.stdout)
    }
}
fn argv(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).into()).collect()
}
const CONTAINER_FORMAT: &str = r#"{"id":{{json .Id}},"name":{{json .Name}},"image":{{json .Image}},"running":{{json .State.Running}},"labels":{{json .Config.Labels}},"memory":{{.HostConfig.Memory}},"swap":{{.HostConfig.MemorySwap}},"cpus":{{.HostConfig.NanoCpus}},"pids":{{.HostConfig.PidsLimit}},"readonly":{{.HostConfig.ReadonlyRootfs}},"privileged":{{.HostConfig.Privileged}},"user":{{json .Config.User}},"network":{{json .HostConfig.NetworkMode}},"caps":{{json .HostConfig.CapDrop}},"security":{{json .HostConfig.SecurityOpt}},"mounts":{{json .Mounts}},"binds":{{if .HostConfig.Binds}}{{json .HostConfig.Binds}}{{else}}[]{{end}},"devices":{{if .HostConfig.Devices}}{{json .HostConfig.Devices}}{{else}}[]{{end}},"restart":{{json .HostConfig.RestartPolicy.Name}}}"#;
pub fn create_arguments(record: &ComputerRecord) -> Vec<String> {
    let spec = &record.spec;
    let mut args = argv(&[
        "container",
        "create",
        "--pull=never",
        "--name",
        &record.container,
        "--read-only",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges:true",
        "--network=none",
        "--user=65532:65532",
        "--ipc=private",
        "--cgroupns=private",
        "--restart=no",
        "--log-driver=none",
        "--stop-signal=SIGKILL",
        "--ulimit=nofile=256:256",
        "--ulimit=core=0:0",
        "--hostname=namzu-pal",
    ]);
    args.extend([
        "--memory".into(),
        spec.memory_bytes.to_string(),
        "--memory-swap".into(),
        spec.memory_bytes.to_string(),
        "--cpus".into(),
        format!("{}.{:03}", spec.cpu_millis / 1000, spec.cpu_millis % 1000),
        "--pids-limit".into(),
        spec.pids.to_string(),
        "--mount".into(),
        format!("type=volume,src={},dst=/workspace", record.volume),
        "--tmpfs".into(),
        "/tmp:rw,nosuid,nodev,size=16777216,mode=1777".into(),
        "--workdir=/workspace".into(),
        format!("--entrypoint={GUEST}"),
    ]);
    for (key, value) in labels(record) {
        args.extend(["--label".into(), format!("{key}={value}")]);
    }
    args.extend([spec.image.clone(), "idle".into()]);
    args
}
impl Engine for Docker {
    fn perform(
        &mut self,
        operation: Operation,
        context: &TurnContext,
    ) -> Result<Answer, AgentError> {
        context.remaining()?;
        match operation {
            Operation::Image(image) => {
                let text = self.command(
                    argv(&[
                        "image",
                        "inspect",
                        "--format",
                        "{{json .Config.Labels}}",
                        &image,
                    ]),
                    &[],
                    context,
                )?;
                let labels: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|_| AgentError::Protocol("invalid Docker image metadata"))?;
                Ok(Answer::Image(
                    labels
                        .get("io.namzu.guest.protocol")
                        .and_then(|v| v.as_str())
                        == Some("1"),
                ))
            }
            Operation::Inspect(name) => {
                let found = self.command(
                    argv(&[
                        "container",
                        "ls",
                        "-a",
                        "--filter",
                        &format!("name=^/{name}$"),
                        "--format",
                        "{{.ID}}",
                    ]),
                    &[],
                    context,
                )?;
                if found.trim().is_empty() {
                    return Ok(Answer::Container(None));
                }
                let text = self.command(
                    argv(&["container", "inspect", "--format", CONTAINER_FORMAT, &name]),
                    &[],
                    context,
                )?;
                let info = serde_json::from_str(&text)
                    .map_err(|_| AgentError::Protocol("invalid owned-container metadata"))?;
                Ok(Answer::Container(Some(info)))
            }
            Operation::VolumeInspect(name) => {
                let found = self.command(
                    argv(&[
                        "volume",
                        "ls",
                        "--filter",
                        &format!("name=^{name}$"),
                        "--format",
                        "{{.Name}}",
                    ]),
                    &[],
                    context,
                )?;
                if found.trim().is_empty() {
                    return Ok(Answer::Volume(None));
                }
                let text = self.command(
                    argv(&[
                        "volume",
                        "inspect",
                        "--format",
                        r#"{"name":{{json .Name}},"labels":{{json .Labels}}}"#,
                        &name,
                    ]),
                    &[],
                    context,
                )?;
                Ok(Answer::Volume(Some(serde_json::from_str(&text).map_err(
                    |_| AgentError::Protocol("invalid owned-volume metadata"),
                )?)))
            }
            Operation::CreateVolume { record } => {
                let mut args = argv(&["volume", "create"]);
                for (k, v) in labels(&record) {
                    args.extend(["--label".into(), format!("{k}={v}")]);
                }
                args.push(record.volume);
                self.command(args, &[], context)?;
                Ok(Answer::Done)
            }
            Operation::Create { record } => {
                self.command(create_arguments(&record), &[], context)?;
                Ok(Answer::Done)
            }
            Operation::Start(id) => {
                self.command(argv(&["container", "start", &id]), &[], context)?;
                Ok(Answer::Done)
            }
            Operation::Kill(id) => {
                self.command(
                    argv(&["container", "kill", "--signal=KILL", &id]),
                    &[],
                    context,
                )?;
                Ok(Answer::Done)
            }
            Operation::Remove(id) => {
                self.command(argv(&["container", "rm", "--force", &id]), &[], context)?;
                Ok(Answer::Done)
            }
            Operation::RemoveVolume(name) => {
                self.command(argv(&["volume", "rm", &name]), &[], context)?;
                Ok(Answer::Done)
            }
            Operation::Exec { container, request } => {
                let input = serde_json::to_vec(&request)
                    .map_err(|_| AgentError::Protocol("cannot encode guest request"))?;
                let text = self.command(
                    argv(&[
                        "container",
                        "exec",
                        "--interactive",
                        &container,
                        GUEST,
                        "request",
                    ]),
                    &input,
                    context,
                )?;
                Ok(Answer::Guest(serde_json::from_str(&text).map_err(
                    |_| AgentError::Protocol("invalid guest response; computer must stop"),
                )?))
            }
        }
    }
}
impl ContainerInfo {
    pub(crate) fn confined(&self, spec: &ComputerSpec, volume: &str) -> bool {
        self.image == spec.image
            && self.memory == spec.memory_bytes
            && self.swap == spec.memory_bytes as i64
            && self.cpus == u64::from(spec.cpu_millis) * 1_000_000
            && self.pids == i64::from(spec.pids)
            && self.readonly
            && !self.privileged
            && self.user == "65532:65532"
            && self.network == "none"
            && self.restart == "no"
            && self.caps.iter().any(|v| v == "ALL")
            && self.security.iter().any(|s| s == "no-new-privileges:true")
            && self.binds.is_empty()
            && self.devices.is_empty()
            && self.mounts.len() == 1
            && self.mounts[0].kind == "volume"
            && self.mounts[0].name == volume
            && self.mounts[0].destination == "/workspace"
    }
}
