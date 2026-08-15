use crate::codex_auth::PrivateCodexHome;
use crate::codex_package::CachedCodexPackage;
use crate::model_candidate::ModelExecution;
use crate::task::TaskInfo;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static CODEX_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const CONTAINER_HOME_PARENT: &str = "/run/a3s-codex";
const CONTAINER_HOME: &str = "/run/a3s-codex/home";
const CONTAINER_CODEX_HOME: &str = "/run/a3s-codex/home/.codex";
const CONTAINER_PACKAGE_PARENT: &str = "/opt/a3s";
const CONTAINER_PACKAGE: &str = "/opt/a3s/codex";
const FALLBACK_CONTAINER_WORKSPACE: &str = "/workspace";
const STAGED_CONTAINER_WORKSPACE: &str = "/workspace/tree";
const PROXY_HELPER_IMAGE: &str =
    "python@sha256:6d43704baacd1bfbe7c295d7f13079d5d8104ed33568873133f8fc69980419df";
const PROXY_STAGING_PARENT: &str = "/run/a3s-proxy-tools";
const PROXY_CONTAINER_ROOT: &str = "/opt/a3s-proxy";
const PROXY_SCRIPT_NAME: &str = "codex_connect_proxy.py";
const PROXY_PORT: u16 = 3128;
const MAX_STDOUT_CAPTURE: usize = 4 * 1024 * 1024;
const MAX_STDERR_CAPTURE: usize = 512 * 1024;
const CONTAINER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(15);
const CONTAINER_CLEANUP_RETRY_TIMEOUT: Duration = Duration::from_secs(600);
const CONTAINER_MUTATION_SETTLE_TIMEOUT: Duration = Duration::from_secs(60);
const CONTAINER_CLEANUP_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const CONTAINER_STAGING_TIMEOUT: Duration = Duration::from_secs(600);
const CONTAINER_OWNER_LABEL: &str = "a3s.bench.codex.owner";
const CONTAINER_RUN_LABEL: &str = "a3s.bench.codex.run";
const CONTAINER_BOOT_ID_LABEL: &str = "a3s.bench.codex.boot_id";
const CONTAINER_PID_LABEL: &str = "a3s.bench.codex.pid";
const CONTAINER_PID_START_LABEL: &str = "a3s.bench.codex.pid_start_ticks";
const CONTAINER_CREATED_AT_LABEL: &str = "a3s.bench.codex.created_at";
const PENDING_MUTATION_STABLE_ZERO: Duration = Duration::from_secs(5);

pub enum CodexOutcome {
    Completed(Option<ModelExecution>),
    TimedOut,
}

pub struct CodexExecutionRequest<'a> {
    pub task: &'a TaskInfo,
    pub package: &'a CachedCodexPackage,
    pub workspace: &'a Path,
    pub seed_workspace: Option<&'a Path>,
    pub instructions: &'a str,
    pub task_prompt: &'a str,
    pub model: Option<&'a str>,
    pub reasoning_effort: Option<&'a str>,
    pub timeout_sec: u64,
    pub state_root: &'a Path,
    pub event_log: Option<&'a Path>,
}

#[derive(Clone)]
struct CodexResources {
    main_container: String,
    staging_container: String,
    package_volume: String,
    home_volume: String,
    workspace_volume: String,
    proxy_container: Option<String>,
    internal_network: Option<String>,
    proxy_volume: Option<String>,
    lifecycle: Arc<CodexLifecycle>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RunMetadata {
    run_id: String,
    boot_id: String,
    pid: u32,
    pid_start_ticks: u64,
    created_at: u64,
}

#[derive(Debug)]
struct CodexLifecycle {
    metadata: RunMetadata,
    pending_mutation: AtomicBool,
}

impl CodexResources {
    #[cfg(test)]
    fn new(main_container: String, restricted_network: bool) -> Self {
        let metadata = RunMetadata::current(main_container.clone()).unwrap();
        Self::with_metadata(main_container, restricted_network, metadata)
    }

    fn with_metadata(
        main_container: String,
        restricted_network: bool,
        metadata: RunMetadata,
    ) -> Self {
        let proxy_container = restricted_network.then(|| format!("{main_container}-proxy"));
        let internal_network = restricted_network.then(|| format!("{main_container}-internal"));
        let proxy_volume = restricted_network.then(|| format!("{main_container}-proxy-tools"));
        let lifecycle = Arc::new(CodexLifecycle {
            metadata,
            pending_mutation: AtomicBool::new(false),
        });
        Self {
            staging_container: format!("{main_container}-stage"),
            package_volume: format!("{main_container}-package"),
            home_volume: format!("{main_container}-home"),
            workspace_volume: format!("{main_container}-workspace"),
            proxy_container,
            internal_network,
            proxy_volume,
            main_container,
            lifecycle,
        }
    }

    fn volumes(&self) -> Vec<&str> {
        let mut volumes = vec![
            self.package_volume.as_str(),
            self.home_volume.as_str(),
            self.workspace_volume.as_str(),
        ];
        volumes.extend(self.proxy_volume.as_deref());
        volumes
    }

    fn metadata(&self) -> &RunMetadata {
        &self.lifecycle.metadata
    }

    fn mark_pending_mutation(&self) {
        self.lifecycle
            .pending_mutation
            .store(true, Ordering::SeqCst);
    }

    fn has_pending_mutation(&self) -> bool {
        self.lifecycle.pending_mutation.load(Ordering::SeqCst)
    }
}

impl RunMetadata {
    fn current(run_id: String) -> Result<Self> {
        anyhow::ensure!(!run_id.is_empty(), "Codex Docker run id is empty");
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .context("could not read the host boot id for Codex Docker ownership")?
            .trim()
            .to_owned();
        anyhow::ensure!(
            !boot_id.is_empty()
                && boot_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "host boot id is invalid"
        );
        let pid = std::process::id();
        let pid_start_ticks = read_pid_start_ticks(pid)?
            .ok_or_else(|| anyhow::anyhow!("current process disappeared while reading /proc"))?;
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before the Unix epoch")?
            .as_secs();
        Ok(Self {
            run_id,
            boot_id,
            pid,
            pid_start_ticks,
            created_at,
        })
    }

    fn add_labels(&self, command: &mut Command, owner: &str) {
        for (key, value) in [
            (CONTAINER_OWNER_LABEL, owner.to_owned()),
            (CONTAINER_RUN_LABEL, self.run_id.clone()),
            (CONTAINER_BOOT_ID_LABEL, self.boot_id.clone()),
            (CONTAINER_PID_LABEL, self.pid.to_string()),
            (CONTAINER_PID_START_LABEL, self.pid_start_ticks.to_string()),
            (CONTAINER_CREATED_AT_LABEL, self.created_at.to_string()),
        ] {
            command.arg("--label").arg(format!("{key}={value}"));
        }
    }

    fn from_labels(resource: &str, labels: &serde_json::Map<String, Value>) -> Result<Self> {
        let label = |key: &str| -> Result<&str> {
            labels
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "refusing to sweep Codex resource {resource}: missing metadata label {key}"
                    )
                })
        };
        let run_id = label(CONTAINER_RUN_LABEL)?.to_owned();
        let boot_id = label(CONTAINER_BOOT_ID_LABEL)?.to_owned();
        let pid = label(CONTAINER_PID_LABEL)?.parse().with_context(|| {
            format!("Codex resource {resource} has an invalid pid metadata label")
        })?;
        let pid_start_ticks = label(CONTAINER_PID_START_LABEL)?.parse().with_context(|| {
            format!("Codex resource {resource} has an invalid pid start metadata label")
        })?;
        let created_at = label(CONTAINER_CREATED_AT_LABEL)?
            .parse()
            .with_context(|| {
                format!("Codex resource {resource} has an invalid created-at metadata label")
            })?;
        Ok(Self {
            run_id,
            boot_id,
            pid,
            pid_start_ticks,
            created_at,
        })
    }
}

fn parse_proc_stat_start_ticks(stat: &str) -> Result<u64> {
    let command_end = stat
        .rfind(')')
        .ok_or_else(|| anyhow::anyhow!("process stat omitted the command terminator"))?;
    stat[command_end + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| anyhow::anyhow!("process stat omitted the start time"))?
        .parse()
        .context("process stat start time was invalid")
}

fn read_pid_start_ticks(pid: u32) -> Result<Option<u64>> {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => parse_proc_stat_start_ticks(&stat).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("could not read /proc/{pid}/stat")),
    }
}

fn run_metadata_is_active<F>(
    metadata: &RunMetadata,
    current_boot_id: &str,
    mut pid_start: F,
) -> Result<bool>
where
    F: FnMut(u32) -> Result<Option<u64>>,
{
    if metadata.boot_id != current_boot_id {
        return Ok(false);
    }
    Ok(pid_start(metadata.pid)? == Some(metadata.pid_start_ticks))
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum DockerResourceKind {
    Container,
    Network,
    Volume,
}

#[derive(Debug)]
struct DiscoveredDockerResource {
    kind: DockerResourceKind,
    name: String,
    metadata: RunMetadata,
}

fn list_run_labeled_resources(kind: DockerResourceKind) -> Result<Vec<String>> {
    let mut command = Command::new("docker");
    match kind {
        DockerResourceKind::Container => command.args(["container", "ls", "-a"]),
        DockerResourceKind::Network => command.args(["network", "ls"]),
        DockerResourceKind::Volume => command.args(["volume", "ls"]),
    };
    command
        .arg("--filter")
        .arg(format!("label={CONTAINER_RUN_LABEL}"))
        .arg("--format")
        .arg(match kind {
            DockerResourceKind::Container => "{{.Names}}",
            DockerResourceKind::Network | DockerResourceKind::Volume => "{{.Name}}",
        });
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not enumerate stale Codex Docker resources")?;
    anyhow::ensure!(
        !timed_out,
        "Codex Docker stale-resource enumeration timed out"
    );
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "Codex Docker stale-resource enumeration failed: {}",
        cleanup_diagnostics(&output)
    );
    let output = String::from_utf8(output.stdout)
        .context("Codex Docker stale-resource names were not UTF-8")?;
    let names = output
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(names)
}

fn inspect_resource_labels(
    kind: DockerResourceKind,
    name: &str,
) -> Result<Option<serde_json::Map<String, Value>>> {
    let mut command = Command::new("docker");
    match kind {
        DockerResourceKind::Container => command.args([
            "container",
            "inspect",
            "--format",
            "{{json .Config.Labels}}",
        ]),
        DockerResourceKind::Network => {
            command.args(["network", "inspect", "--format", "{{json .Labels}}"])
        }
        DockerResourceKind::Volume => {
            command.args(["volume", "inspect", "--format", "{{json .Labels}}"])
        }
    };
    command.arg(name);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not inspect stale Codex Docker resource metadata")?;
    anyhow::ensure!(!timed_out, "Codex Docker metadata inspection timed out");
    if !output.status.success() {
        if resource_inspect_diagnostics_are_missing(kind, &cleanup_diagnostics(&output)) {
            return Ok(None);
        }
        anyhow::bail!(
            "Codex Docker metadata inspection failed for {name}: {}",
            cleanup_diagnostics(&output)
        );
    }
    anyhow::ensure!(
        !output.stdout_truncated && !output.stderr_truncated,
        "Codex Docker metadata inspection diagnostics were truncated for {name}"
    );
    let value: Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("Codex Docker metadata for {name} was malformed"))?;
    value
        .as_object()
        .cloned()
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("Codex Docker metadata for {name} was not an object"))
}

fn resource_inspect_diagnostics_are_missing(kind: DockerResourceKind, diagnostics: &str) -> bool {
    let diagnostics = diagnostics.to_ascii_lowercase();
    match kind {
        DockerResourceKind::Container => {
            diagnostics.contains("no such object") || diagnostics.contains("no such container")
        }
        DockerResourceKind::Network => {
            diagnostics.contains("no such network") || diagnostics.contains("not found")
        }
        DockerResourceKind::Volume => diagnostics.contains("no such volume"),
    }
}

fn discover_run_labeled_resources() -> Result<Vec<DiscoveredDockerResource>> {
    let mut resources = Vec::new();
    for kind in [
        DockerResourceKind::Container,
        DockerResourceKind::Network,
        DockerResourceKind::Volume,
    ] {
        for name in list_run_labeled_resources(kind)? {
            let Some(labels) = inspect_resource_labels(kind, &name)? else {
                continue;
            };
            let owner = labels
                .get(CONTAINER_OWNER_LABEL)
                .and_then(Value::as_str)
                .unwrap_or_default();
            anyhow::ensure!(
                owner == name,
                "refusing to sweep Codex resource {name}: ownership label mismatch"
            );
            let metadata = RunMetadata::from_labels(&name, &labels)?;
            resources.push(DiscoveredDockerResource {
                kind,
                name,
                metadata,
            });
        }
    }
    Ok(resources)
}

fn sweep_stale_codex_resources(current: &RunMetadata) -> Result<()> {
    let resources = discover_run_labeled_resources()?;
    let mut groups: BTreeMap<String, (RunMetadata, Vec<DiscoveredDockerResource>)> =
        BTreeMap::new();
    for resource in resources {
        let run_id = resource.metadata.run_id.clone();
        let entry = groups
            .entry(run_id.clone())
            .or_insert_with(|| (resource.metadata.clone(), Vec::new()));
        anyhow::ensure!(
            entry.0 == resource.metadata,
            "refusing to sweep Codex run {run_id}: resource metadata is inconsistent"
        );
        entry.1.push(resource);
    }
    for (run_id, (metadata, mut resources)) in groups {
        if run_metadata_is_active(&metadata, &current.boot_id, read_pid_start_ticks)? {
            continue;
        }
        resources.sort_by_key(|resource| resource.kind);
        for resource in resources {
            match resource.kind {
                DockerResourceKind::Container => remove_container(&resource.name),
                DockerResourceKind::Network => remove_internal_network(&resource.name),
                DockerResourceKind::Volume => remove_volume(&resource.name),
            }
            .with_context(|| {
                format!(
                    "could not sweep stale Codex run {run_id} resource {}",
                    resource.name
                )
            })?;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct StableZeroWindow {
    required: Duration,
    since: Option<Instant>,
}

impl StableZeroWindow {
    fn new(required: Duration) -> Self {
        Self {
            required,
            since: None,
        }
    }

    fn observe(&mut self, now: Instant, all_absent: bool) -> bool {
        if !all_absent {
            self.since = None;
            return false;
        }
        let since = self.since.get_or_insert(now);
        now.duration_since(*since) >= self.required
    }
}

type ResourceRemover = Box<dyn FnMut(&CodexResources) -> Result<()> + 'static>;

struct CodexRunGuard {
    resources: CodexResources,
    private_home: Option<PrivateCodexHome>,
    remover: ResourceRemover,
    resources_removed: bool,
    finished: bool,
}

impl CodexRunGuard {
    fn new(resources: CodexResources, private_home: PrivateCodexHome) -> Self {
        Self::with_remover(resources, private_home, Box::new(remove_resources))
    }

    fn with_remover(
        resources: CodexResources,
        private_home: PrivateCodexHome,
        remover: ResourceRemover,
    ) -> Self {
        Self {
            resources,
            private_home: Some(private_home),
            remover,
            resources_removed: false,
            finished: false,
        }
    }

    #[cfg(test)]
    fn with_test_remover<F>(container: String, private_home: PrivateCodexHome, remover: F) -> Self
    where
        F: FnMut(&str) -> Result<()> + 'static,
    {
        let resources = CodexResources::new(container, false);
        let mut remover = remover;
        Self::with_remover(
            resources,
            private_home,
            Box::new(move |resources| remover(&resources.main_container)),
        )
    }

    fn confirm_container_removed(&mut self) -> Result<()> {
        if self.resources_removed {
            return Ok(());
        }
        (self.remover)(&self.resources).context("could not remove Codex Docker resources")?;
        self.resources_removed = true;
        Ok(())
    }

    fn cleanup_private_home(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.resources_removed,
            "cannot clean up the private Codex home before Docker resource removal is confirmed"
        );
        let result = self
            .private_home
            .as_mut()
            .expect("Codex auth guard is live")
            .cleanup();
        if result.is_ok() {
            self.finished = true;
            self.private_home.take();
        }
        result
    }

    fn finish(mut self) -> Result<()> {
        self.confirm_container_removed()?;
        self.cleanup_private_home()
    }
}

impl Drop for CodexRunGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Err(error) = self.confirm_container_removed() {
            if let Some(private_home) = self.private_home.as_mut() {
                private_home.retain_for_stale_recovery();
            }
            eprintln!(
                "warning: could not confirm removal of Codex Docker resources; retaining private Codex home for stale recovery: {error:#}"
            );
            return;
        }
        if let Err(error) = self.cleanup_private_home() {
            eprintln!(
                "warning: Codex Docker resource removal was confirmed, but private Codex home cleanup failed: {error:#}"
            );
        }
    }
}

pub fn execute(request: CodexExecutionRequest<'_>) -> Result<CodexOutcome> {
    let main_container = container_name();
    let metadata = RunMetadata::current(main_container.clone())?;
    sweep_stale_codex_resources(&metadata)?;
    crate::codex_package::validate_platform(
        request.package.target_triple(),
        request.task.work_platform.as_deref(),
    )?;

    let restricted_network = task_uses_restricted_network(request.task)?;
    let resources = CodexResources::with_metadata(main_container, restricted_network, metadata);
    let private_home = crate::codex_auth::stage(request.state_root, None)?;
    private_home.prepare_for_container_copy()?;
    let container_workspace = container_workspace(request.task)?;
    let guard = CodexRunGuard::new(resources.clone(), private_home);
    let prompt = format!(
        "{}\n\n# Benchmark task\n\n{}\n\nWork only in the supplied workspace and complete the task.",
        request.instructions, request.task_prompt
    );
    let result = (|| -> Result<CodexOutcome> {
        let mut create =
            build_codex_run_command(&request, &prompt, &resources, &container_workspace)?;
        let proxy_asset = restricted_network
            .then(|| stage_proxy_runtime_asset(request.state_root))
            .transpose()?;
        if restricted_network {
            ensure_proxy_helper_image()?;
            create_internal_network(&resources)?;
        }
        for volume in resources.volumes() {
            create_volume(&resources, volume)?;
        }
        create_staging_container(request.task, &resources, request.seed_workspace.is_some())?;
        let staging_id = owned_container_id(&resources.staging_container)?
            .ok_or_else(|| anyhow::anyhow!("Codex staging container disappeared"))?;
        stage_container_inputs(
            &staging_id,
            &request,
            guard
                .private_home
                .as_ref()
                .expect("Codex auth guard is live"),
            proxy_asset.as_deref(),
        )?;
        let (created, create_timed_out) =
            output_with_timeout(&mut create, CONTAINER_STAGING_TIMEOUT)
                .context("could not create containerized Codex Candidate")?;
        if create_timed_out {
            resources.mark_pending_mutation();
            wait_for_delayed_container(&resources.main_container)?;
            anyhow::bail!("Codex container creation timed out");
        }
        anyhow::ensure!(
            created.status.success() && !created.stdout_truncated && !created.stderr_truncated,
            "could not create containerized Codex Candidate: {}",
            cleanup_diagnostics(&created)
        );
        let container_id = owned_container_id(&resources.main_container)?
            .ok_or_else(|| anyhow::anyhow!("Codex container disappeared during staging"))?;
        if restricted_network {
            start_proxy_sidecar(&resources)?;
        }
        let mut command = Command::new("docker");
        command.args(["start", "--attach", &container_id]);
        let (output, timed_out) =
            output_with_timeout(&mut command, Duration::from_secs(request.timeout_sec))
                .context("could not attach to containerized Codex Candidate")?;
        if timed_out {
            stop_container_process(&resources.main_container)?;
        }
        let private_home = guard
            .private_home
            .as_ref()
            .expect("Codex auth guard is live");
        let events = private_home.redact(&output.stdout);
        let diagnostics = private_home.redact(&output.stderr);
        persist_events(request.event_log, &events)?;
        crate::workspace::export_container_tree(
            &container_id,
            &container_workspace,
            request.workspace,
        )
        .context("could not export the containerized Codex workspace")?;
        if timed_out {
            return Ok(CodexOutcome::TimedOut);
        }
        anyhow::ensure!(
            !output.stdout_truncated && !output.stderr_truncated,
            "Codex Candidate emitted too much output"
        );
        if !output.status.success() {
            anyhow::bail!(
                "Codex Candidate exited with {}: {}",
                output.status,
                failure_diagnostics(
                    &String::from_utf8_lossy(&events),
                    &String::from_utf8_lossy(&diagnostics)
                )
            );
        }
        Ok(CodexOutcome::Completed(parse_usage(
            &String::from_utf8_lossy(&events),
        )?))
    })();
    let cleanup = guard.finish();
    match (result, cleanup) {
        (Err(error), Err(cleanup_error)) => Err(error.context(format!(
            "could not clean up containerized Codex after Candidate failure: {cleanup_error:#}"
        ))),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error.context("could not clean up containerized Codex")),
        (Ok(outcome), Ok(())) => Ok(outcome),
    }
}

fn build_codex_run_command(
    request: &CodexExecutionRequest<'_>,
    prompt: &str,
    resources: &CodexResources,
    container_workspace: &str,
) -> Result<Command> {
    if let Some(model) = request.model {
        crate::lock::validate_codex_model(model)?;
    }
    if let Some(reasoning_effort) = request.reasoning_effort {
        crate::lock::validate_reasoning_effort(reasoning_effort)?;
    }
    let paths = request.package.container_paths()?;
    anyhow::ensure!(
        paths.entrypoint == "bin/codex",
        "Codex entrypoint is not at the official package path"
    );
    let entrypoint = container_path(&paths.entrypoint);
    let mut command = Command::new("docker");
    add_production_container_args(
        &mut command,
        request.task,
        resources,
        container_workspace,
        request.seed_workspace.is_some(),
    )?;
    add_codex_environment(&mut command, request.package)?;
    add_proxy_environment(&mut command, resources);
    request.package.verify_for_mount()?;
    command
        .arg("--mount")
        .arg(format!(
            "type=volume,src={},dst={CONTAINER_PACKAGE},volume-subpath=codex,readonly",
            resources.package_volume
        ))
        .arg("--mount")
        .arg(format!(
            "type=volume,src={},dst={CONTAINER_HOME},volume-subpath=home",
            resources.home_volume
        ))
        .arg("--workdir")
        .arg(container_workspace)
        .arg("--entrypoint")
        .arg(entrypoint)
        .arg(&request.task.work_image)
        .args(codex_argv(request, prompt, container_workspace));
    Ok(command)
}

fn stage_container_inputs(
    container_id: &str,
    request: &CodexExecutionRequest<'_>,
    private_home: &PrivateCodexHome,
    proxy_asset: Option<&Path>,
) -> Result<()> {
    anyhow::ensure!(
        private_home.codex_path().parent() == Some(private_home.path()),
        "private Codex home layout is invalid"
    );
    copy_tree_into_container(container_id, &request.package.root, CONTAINER_PACKAGE)
        .context("could not stage the Codex package in its private volume")?;
    copy_tree_into_container(container_id, private_home.path(), CONTAINER_HOME)
        .context("could not stage the private Codex home in its private volume")?;
    if let Some(seed_workspace) = request.seed_workspace {
        prepare_workspace_for_container_copy(seed_workspace)?;
        copy_tree_into_container(container_id, seed_workspace, STAGED_CONTAINER_WORKSPACE)
            .context("could not stage the materialized workspace in its private volume")?;
    }
    if let Some(asset) = proxy_asset {
        copy_tree_into_container(
            container_id,
            asset,
            &format!("{PROXY_STAGING_PARENT}/{PROXY_SCRIPT_NAME}"),
        )
        .context("could not stage the embedded Codex CONNECT proxy")?;
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_workspace_for_container_copy(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            prepare_workspace_for_container_copy(&entry.path())?;
        } else if kind.is_file() {
            let metadata = entry.metadata()?;
            let mode = if metadata.permissions().mode() & 0o111 != 0 {
                0o777
            } else {
                0o666
            };
            std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(mode))?;
        } else {
            anyhow::bail!("materialized Codex workspace contains a special file");
        }
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o777))?;
    Ok(())
}

#[cfg(not(unix))]
fn prepare_workspace_for_container_copy(_path: &Path) -> Result<()> {
    anyhow::bail!("containerized Codex requires Unix workspace permissions")
}

fn copy_tree_into_container(container_id: &str, source: &Path, destination: &str) -> Result<()> {
    let source = source
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Docker copy source must be UTF-8"))?;
    anyhow::ensure!(
        !source.is_empty()
            && !source.contains(':')
            && !source.contains('\0')
            && !source.chars().any(char::is_control),
        "Docker copy source contains an unsafe character"
    );
    anyhow::ensure!(
        destination.starts_with('/')
            && !destination.contains(':')
            && !destination.contains('\0')
            && !destination.chars().any(char::is_control),
        "Docker copy destination contains an unsafe character"
    );
    let mut command = Command::new("docker");
    command
        .args(["cp", "--archive"])
        .arg(source)
        .arg(format!("{container_id}:{destination}"));
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_STAGING_TIMEOUT)
        .context("could not run bounded Docker input staging")?;
    anyhow::ensure!(!timed_out, "Docker input staging timed out");
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "Docker input staging failed: {}",
        cleanup_diagnostics(&output)
    );
    Ok(())
}

fn codex_argv(
    request: &CodexExecutionRequest<'_>,
    prompt: &str,
    container_workspace: &str,
) -> Vec<String> {
    let mut argv = vec![
        "exec".into(),
        "--dangerously-bypass-approvals-and-sandbox".into(),
        "--cd".into(),
        container_workspace.into(),
        "--ephemeral".into(),
        "--json".into(),
        "--skip-git-repo-check".into(),
        "--ignore-user-config".into(),
        "--ignore-rules".into(),
        "--color".into(),
        "never".into(),
        "-c".into(),
        "shell_environment_policy.inherit=all".into(),
        "-c".into(),
        "shell_environment_policy.ignore_default_excludes=false".into(),
        "-c".into(),
        r#"shell_environment_policy.exclude=["HTTP_PROXY","HTTPS_PROXY","ALL_PROXY","NO_PROXY","http_proxy","https_proxy","all_proxy","no_proxy","CODEX_HOME","CODEX_CODE_MODE_HOST_PATH"]"#.into(),
    ];
    if let Some(model) = request.model {
        argv.extend(["--model".into(), model.into()]);
    }
    if let Some(reasoning_effort) = request.reasoning_effort {
        argv.extend([
            "-c".into(),
            format!("model_reasoning_effort={reasoning_effort}"),
        ]);
    }
    argv.extend(["--".into(), prompt.into()]);
    argv
}

fn container_path(relative: &str) -> String {
    format!("{CONTAINER_PACKAGE}/{relative}")
}

fn task_uses_restricted_network(task: &TaskInfo) -> Result<bool> {
    match task.work_network_need.as_str() {
        "none" => Ok(true),
        "public_internet" => Ok(false),
        value => anyhow::bail!("unsupported Codex Task network need {value:?}"),
    }
}

fn stage_proxy_runtime_asset(state_root: &Path) -> Result<PathBuf> {
    let asset_root = state_root.join("runtime-assets");
    let script = asset_root.join(PROXY_SCRIPT_NAME);
    let bytes = include_bytes!("../runtime_assets/codex_connect_proxy.py");
    crate::state_fs::secure_atomic_write(&script, bytes)
        .context("could not stage the embedded Codex CONNECT proxy asset")?;
    let mut permissions = std::fs::metadata(&script)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o444);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(true);
    std::fs::set_permissions(&script, permissions)?;
    anyhow::ensure!(
        crate::state_fs::read_regular_file(&script, "Codex CONNECT proxy asset")? == bytes,
        "staged Codex CONNECT proxy asset does not match the embedded component"
    );
    Ok(script)
}

fn add_proxy_environment(command: &mut Command, resources: &CodexResources) {
    let Some(proxy) = resources.proxy_container.as_deref() else {
        return;
    };
    let endpoint = format!("http://{proxy}:{PROXY_PORT}");
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.arg("--env").arg(format!("{name}={endpoint}"));
    }
    command.args(["--env", "NO_PROXY=", "--env", "no_proxy="]);
}

fn ensure_proxy_helper_image() -> Result<()> {
    let mut command = Command::new("docker");
    command
        .args(["image", "inspect", "--format"])
        .arg("{{json .RepoDigests}}\t{{.Os}}\t{{.Architecture}}")
        .arg(PROXY_HELPER_IMAGE);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not inspect the local Codex proxy helper image")?;
    anyhow::ensure!(!timed_out, "Codex proxy helper image inspection timed out");
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "required local Codex proxy helper image {PROXY_HELPER_IMAGE} is unavailable; load it before running network-isolated Tasks: {}",
        cleanup_diagnostics(&output)
    );
    let value = String::from_utf8(output.stdout)
        .context("Codex proxy helper image identity was not UTF-8")?;
    let mut fields = value.trim().split('\t');
    let digests = fields.next().unwrap_or_default();
    let os = fields.next().unwrap_or_default();
    let architecture = fields.next().unwrap_or_default();
    anyhow::ensure!(
        fields.next().is_none(),
        "Codex proxy helper image identity was malformed"
    );
    let digests: Vec<String> =
        serde_json::from_str(digests).context("Codex proxy helper RepoDigests were malformed")?;
    anyhow::ensure!(
        digests.iter().any(|digest| digest == PROXY_HELPER_IMAGE),
        "local Codex proxy helper image does not match the required immutable digest"
    );
    anyhow::ensure!(
        os == "linux" && !architecture.is_empty(),
        "Codex proxy helper image must be a concrete Linux platform image"
    );
    Ok(())
}

fn add_codex_environment(command: &mut Command, package: &CachedCodexPackage) -> Result<()> {
    let paths = package.container_paths()?;
    command
        .arg("--env")
        .arg(format!("HOME={CONTAINER_HOME}"))
        .arg("--env");
    command.arg(format!("CODEX_HOME={CONTAINER_CODEX_HOME}"));
    command.arg("--env").arg(format!(
        "CODEX_CODE_MODE_HOST_PATH={}",
        container_path(&paths.code_mode_host)
    ));
    command.args([
        "--env",
        "LANG=C.UTF-8",
        "--env",
        "NO_COLOR=1",
        "--env",
        "MAVEN_OPTS=-Dmaven.repo.local=/run/a3s-codex/home/.m2/repository",
    ]);
    Ok(())
}

fn create_staging_container(
    task: &TaskInfo,
    resources: &CodexResources,
    stage_workspace: bool,
) -> Result<()> {
    let mut command = Command::new("docker");
    command.args([
        "create",
        "--pull",
        "never",
        "--name",
        &resources.staging_container,
    ]);
    resources
        .metadata()
        .add_labels(&mut command, &resources.staging_container);
    command.args([
        "--mount",
        &format!(
            "type=volume,src={},dst={CONTAINER_PACKAGE_PARENT},volume-nocopy",
            resources.package_volume
        ),
        "--mount",
        &format!(
            "type=volume,src={},dst={CONTAINER_HOME_PARENT},volume-nocopy",
            resources.home_volume
        ),
    ]);
    if let Some(proxy_volume) = resources.proxy_volume.as_deref() {
        command.arg("--mount").arg(format!(
            "type=volume,src={proxy_volume},dst={PROXY_STAGING_PARENT},volume-nocopy"
        ));
    }
    if stage_workspace {
        command.arg("--mount").arg(format!(
            "type=volume,src={},dst={FALLBACK_CONTAINER_WORKSPACE},volume-nocopy",
            resources.workspace_volume
        ));
    }
    if let Some(platform) = task.work_platform.as_deref() {
        command.args(["--platform", platform]);
    }
    // Supply a Cmd without starting or executing anything. This also lets
    // Docker create staging containers from images that omit a default Cmd.
    command.arg(&task.work_image).arg("/bin/true");
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_STAGING_TIMEOUT)
        .context("could not create the Codex staging container")?;
    if timed_out {
        resources.mark_pending_mutation();
        wait_for_delayed_container(&resources.staging_container)?;
        anyhow::bail!("Codex staging container creation timed out");
    }
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "could not create the Codex staging container: {}",
        cleanup_diagnostics(&output)
    );
    anyhow::ensure!(
        owned_container_id(&resources.staging_container)?.is_some(),
        "Codex staging container disappeared after creation"
    );
    Ok(())
}

fn add_production_container_args(
    command: &mut Command,
    task: &TaskInfo,
    resources: &CodexResources,
    container_workspace: &str,
    seed_workspace: bool,
) -> Result<()> {
    let restricted = task_uses_restricted_network(task)?;
    anyhow::ensure!(
        restricted == resources.internal_network.is_some()
            && restricted == resources.proxy_container.is_some()
            && restricted == resources.proxy_volume.is_some(),
        "Codex network resources do not match the Task network policy"
    );
    let network = resources.internal_network.as_deref().unwrap_or("bridge");
    add_container_args(
        command,
        task,
        resources,
        container_workspace,
        seed_workspace,
        network,
    )
}

fn add_container_args(
    command: &mut Command,
    task: &TaskInfo,
    resources: &CodexResources,
    container_workspace: &str,
    seed_workspace: bool,
    network: &str,
) -> Result<()> {
    command.args([
        "create",
        "--pull",
        "never",
        "--name",
        &resources.main_container,
        "--read-only",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
    ]);
    resources
        .metadata()
        .add_labels(command, &resources.main_container);
    command.args(crate::runtime_profile::WORK_DOCKER_LIMITS);
    command.args(["--network", network]);
    command.args(["--tmpfs", "/run/a3s-codex:rw,noexec,nosuid,nodev,size=64m"]);
    if let Some(platform) = task.work_platform.as_deref() {
        command.args(["--platform", platform]);
    }
    if seed_workspace {
        command.arg("--mount").arg(format!(
            "type=volume,src={},dst={container_workspace},volume-subpath=tree",
            resources.workspace_volume
        ));
    } else {
        command.arg("--mount").arg(format!(
            "type=volume,src={},dst={container_workspace}",
            resources.workspace_volume
        ));
    }
    Ok(())
}

fn container_workspace(task: &TaskInfo) -> Result<String> {
    let raw_path = if task.root.join("public/workspace").is_dir() {
        FALLBACK_CONTAINER_WORKSPACE
    } else {
        task.workspace_seed
            .as_ref()
            .map(|seed| seed.source_path.as_str())
            .unwrap_or(FALLBACK_CONTAINER_WORKSPACE)
    };
    let path = if raw_path != "/" {
        raw_path.strip_suffix('/').unwrap_or(raw_path)
    } else {
        raw_path
    };
    anyhow::ensure!(
        !raw_path.contains("//")
            && path.starts_with('/')
            && path != "/"
            && !path.ends_with('/')
            && !path.contains("//")
            && !path.contains(',')
            && !path.contains('\0')
            && path
                .split('/')
                .skip(1)
                .all(|component| { !component.is_empty() && !matches!(component, "." | "..") })
            && !path.chars().any(char::is_control),
        "Codex container workspace path is unsafe"
    );
    for reserved in [CONTAINER_HOME, CONTAINER_PACKAGE] {
        anyhow::ensure!(
            path != reserved
                && !path.starts_with(&format!("{reserved}/"))
                && !reserved.starts_with(&format!("{path}/")),
            "Codex container workspace overlaps a reserved path"
        );
    }
    Ok(path.to_owned())
}

fn stop_container_process(container: &str) -> Result<()> {
    let Some(container_id) = owned_container_id(container)? else {
        anyhow::bail!("timed-out Codex container disappeared before workspace export");
    };
    if !container_is_running(&container_id)? {
        return Ok(());
    }
    let mut command = Command::new("docker");
    command.args(["kill", &container_id]);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not stop timed-out Codex container")?;
    anyhow::ensure!(!timed_out, "Codex container stop timed out");
    if !output.status.success() && container_is_running(&container_id)? {
        anyhow::bail!(
            "could not stop timed-out Codex container: {}",
            cleanup_diagnostics(&output)
        );
    }
    anyhow::ensure!(
        !container_is_running(&container_id)?,
        "timed-out Codex container is still running"
    );
    Ok(())
}

struct CommandOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

struct BoundedCapture {
    bytes: Vec<u8>,
    truncated: bool,
}

fn output_with_timeout(command: &mut Command, timeout: Duration) -> Result<(CommandOutput, bool)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Codex stdout pipe was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("Codex stderr pipe was unavailable"))?;
    let stdout_thread = std::thread::spawn(move || drain_bounded(stdout, MAX_STDOUT_CAPTURE));
    let stderr_thread = std::thread::spawn(move || drain_bounded(stderr, MAX_STDERR_CAPTURE));
    let deadline = Instant::now() + timeout;
    let timed_out = loop {
        if child.try_wait()?.is_some() {
            break false;
        }
        if Instant::now() >= deadline {
            if let Err(error) = child.kill() {
                if child.try_wait()?.is_none() {
                    return Err(error.into());
                }
            }
            break true;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let status = child.wait()?;
    let stdout = join_capture(stdout_thread)?;
    let stderr = join_capture(stderr_thread)?;
    Ok((
        CommandOutput {
            status,
            stdout: stdout.bytes,
            stderr: stderr.bytes,
            stdout_truncated: stdout.truncated,
            stderr_truncated: stderr.truncated,
        },
        timed_out,
    ))
}

fn drain_bounded<R: Read>(mut reader: R, limit: usize) -> std::io::Result<BoundedCapture> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut truncated = false;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if bytes.len() < limit {
            let keep = (limit - bytes.len()).min(count);
            bytes.extend_from_slice(&buffer[..keep]);
            truncated |= keep < count;
        } else {
            truncated = true;
        }
    }
    Ok(BoundedCapture { bytes, truncated })
}

fn join_capture(
    thread: std::thread::JoinHandle<std::io::Result<BoundedCapture>>,
) -> Result<BoundedCapture> {
    thread
        .join()
        .map_err(|_| anyhow::anyhow!("Codex output drain thread panicked"))?
        .map_err(Into::into)
}

fn create_internal_network(resources: &CodexResources) -> Result<()> {
    let network = resources
        .internal_network
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Codex internal network resource is unavailable"))?;
    let mut command = Command::new("docker");
    command.args(["network", "create", "--internal"]);
    resources.metadata().add_labels(&mut command, network);
    command.arg(network);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_STAGING_TIMEOUT)
        .context("could not create the private Codex network")?;
    if timed_out {
        resources.mark_pending_mutation();
        wait_for_delayed_network(network)?;
        anyhow::bail!("Codex internal network creation timed out");
    }
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "Codex internal network creation failed: {}",
        cleanup_diagnostics(&output)
    );
    anyhow::ensure!(
        owned_internal_network_exists(network)?,
        "Codex internal network disappeared after creation"
    );
    Ok(())
}

fn owned_internal_network_exists(network: &str) -> Result<bool> {
    let mut command = Command::new("docker");
    command
        .args(["network", "inspect", "--format"])
        .arg("{{.Name}}\t{{ index .Labels \"a3s.bench.codex.owner\" }}\t{{.Internal}}")
        .arg(network);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not inspect Codex network ownership")?;
    anyhow::ensure!(!timed_out, "Docker network ownership inspection timed out");
    if !output.status.success() {
        if missing_network(&output) {
            return Ok(false);
        }
        anyhow::bail!(
            "Docker network ownership inspection failed: {}",
            cleanup_diagnostics(&output)
        );
    }
    anyhow::ensure!(
        !output.stdout_truncated && !output.stderr_truncated,
        "Docker network ownership diagnostics were truncated"
    );
    let value = String::from_utf8(output.stdout)
        .context("Docker network ownership output was not UTF-8")?;
    let mut fields = value.trim().split('\t');
    let name = fields.next().unwrap_or_default();
    let owner = fields.next().unwrap_or_default();
    let internal = fields.next().unwrap_or_default();
    anyhow::ensure!(
        fields.next().is_none() && name == network,
        "Docker network ownership output was malformed"
    );
    anyhow::ensure!(
        owner == network,
        "refusing to operate on Codex network {network}: ownership label mismatch"
    );
    anyhow::ensure!(
        internal == "true",
        "refusing to use Codex network {network}: network is not internal"
    );
    Ok(true)
}

fn missing_network(output: &CommandOutput) -> bool {
    let diagnostics = cleanup_diagnostics(output).to_ascii_lowercase();
    diagnostics.contains("no such network") || diagnostics.contains("not found")
}

fn wait_for_delayed_network(network: &str) -> Result<()> {
    let deadline = Instant::now() + CONTAINER_MUTATION_SETTLE_TIMEOUT;
    loop {
        if owned_internal_network_exists(network)? || Instant::now() >= deadline {
            return Ok(());
        }
        std::thread::sleep(CONTAINER_CLEANUP_RETRY_INTERVAL);
    }
}

fn remove_internal_network(network: &str) -> Result<()> {
    if !owned_internal_network_exists(network)? {
        return Ok(());
    }
    let mut command = Command::new("docker");
    command.args(["network", "rm", network]);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not run bounded Docker network cleanup")?;
    let still_exists = owned_internal_network_exists(network)?;
    if !still_exists {
        return Ok(());
    }
    anyhow::ensure!(!timed_out, "Docker network cleanup timed out");
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "Docker network cleanup failed: {}",
        cleanup_diagnostics(&output)
    );
    anyhow::bail!("Docker internal network still exists after cleanup")
}

fn build_proxy_create_command(resources: &CodexResources) -> Result<Command> {
    let proxy = resources
        .proxy_container
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Codex proxy container resource is unavailable"))?;
    let network = resources
        .internal_network
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Codex internal network resource is unavailable"))?;
    let tools = resources
        .proxy_volume
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Codex proxy tools volume is unavailable"))?;
    let mut command = Command::new("docker");
    command.args(["create", "--pull", "never", "--name", proxy]);
    resources.metadata().add_labels(&mut command, proxy);
    command.args([
        "--read-only",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--user",
        "65534:65534",
        "--pids-limit",
        "128",
        "--memory",
        "256m",
        "--memory-swap",
        "256m",
        "--cpus",
        "0.5",
        "--network",
        network,
        "--env",
        "PYTHONDONTWRITEBYTECODE=1",
        "--mount",
        &format!("type=volume,src={tools},dst={PROXY_CONTAINER_ROOT},readonly"),
        "--entrypoint",
        "python3",
        PROXY_HELPER_IMAGE,
        &format!("{PROXY_CONTAINER_ROOT}/{PROXY_SCRIPT_NAME}"),
        "--bind-internal",
        "--port",
        &PROXY_PORT.to_string(),
    ]);
    Ok(command)
}

fn create_proxy_container(resources: &CodexResources) -> Result<String> {
    let proxy = resources
        .proxy_container
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Codex proxy container resource is unavailable"))?;
    let mut command = build_proxy_create_command(resources)?;
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_STAGING_TIMEOUT)
        .context("could not create the Codex CONNECT proxy")?;
    if timed_out {
        resources.mark_pending_mutation();
        wait_for_delayed_container(proxy)?;
        anyhow::bail!("Codex CONNECT proxy creation timed out");
    }
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "could not create the Codex CONNECT proxy: {}",
        cleanup_diagnostics(&output)
    );
    owned_container_id(proxy)?
        .ok_or_else(|| anyhow::anyhow!("Codex CONNECT proxy disappeared after creation"))
}

fn build_proxy_bridge_connect_command(proxy_id: &str) -> Command {
    let mut command = Command::new("docker");
    command.args(["network", "connect", "bridge", proxy_id]);
    command
}

fn connect_proxy_to_bridge(resources: &CodexResources, proxy_id: &str) -> Result<()> {
    let mut command = build_proxy_bridge_connect_command(proxy_id);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_STAGING_TIMEOUT)
        .context("could not attach the Codex CONNECT proxy to the public bridge")?;
    if timed_out {
        resources.mark_pending_mutation();
    }
    anyhow::ensure!(
        !timed_out,
        "Codex CONNECT proxy public network attachment timed out"
    );
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "could not attach the Codex CONNECT proxy to the public bridge: {}",
        cleanup_diagnostics(&output)
    );
    anyhow::ensure!(
        container_has_network(proxy_id, "bridge")?,
        "Codex CONNECT proxy is not attached to the public bridge"
    );
    Ok(())
}

fn container_has_network(container_id: &str, network: &str) -> Result<bool> {
    let mut command = Command::new("docker");
    command
        .args(["container", "inspect", "--format"])
        .arg(format!(
            "{{{{if index .NetworkSettings.Networks \"{network}\"}}}}true{{{{else}}}}false{{{{end}}}}"
        ))
        .arg(container_id);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not inspect Codex proxy network membership")?;
    anyhow::ensure!(!timed_out, "Codex proxy network inspection timed out");
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "Codex proxy network inspection failed: {}",
        cleanup_diagnostics(&output)
    );
    match String::from_utf8_lossy(&output.stdout).trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        value => anyhow::bail!("Docker returned invalid network membership {value:?}"),
    }
}

fn container_network_ipv4(container_id: &str, network: &str) -> Result<String> {
    let mut command = Command::new("docker");
    command
        .args(["container", "inspect", "--format"])
        .arg(format!(
            "{{{{with index .NetworkSettings.Networks \"{network}\"}}}}{{{{.IPAddress}}}}{{{{end}}}}"
        ))
        .arg(container_id);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not inspect the Codex proxy internal address")?;
    anyhow::ensure!(!timed_out, "Codex proxy address inspection timed out");
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "Codex proxy address inspection failed: {}",
        cleanup_diagnostics(&output)
    );
    let value = String::from_utf8(output.stdout)
        .context("Codex proxy internal address was not UTF-8")?
        .trim()
        .to_owned();
    let address: std::net::IpAddr = value
        .parse()
        .context("Docker returned an invalid Codex proxy internal address")?;
    anyhow::ensure!(
        address.is_ipv4(),
        "Codex proxy internal address is not IPv4"
    );
    Ok(value)
}

fn start_proxy_sidecar(resources: &CodexResources) -> Result<()> {
    let proxy_id = create_proxy_container(resources)?;
    connect_proxy_to_bridge(resources, &proxy_id)?;
    let mut command = Command::new("docker");
    command.args(["start", &proxy_id]);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_STAGING_TIMEOUT)
        .context("could not start the Codex CONNECT proxy")?;
    anyhow::ensure!(!timed_out, "Codex CONNECT proxy startup timed out");
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "could not start the Codex CONNECT proxy: {}",
        cleanup_diagnostics(&output)
    );
    let network = resources
        .internal_network
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Codex internal network resource is unavailable"))?;
    let address = container_network_ipv4(&proxy_id, network)?;
    let deadline = Instant::now() + CONTAINER_MUTATION_SETTLE_TIMEOUT;
    loop {
        anyhow::ensure!(
            container_is_running(&proxy_id)?,
            "Codex CONNECT proxy stopped before becoming ready"
        );
        let mut probe = Command::new("docker");
        probe.args([
            "exec",
            &proxy_id,
            "python3",
            "-c",
            &format!(
                "import socket; socket.create_connection(({address:?}, {PROXY_PORT}), 1).close()"
            ),
        ]);
        let (output, timed_out) = output_with_timeout(&mut probe, CONTAINER_CLEANUP_TIMEOUT)
            .context("could not probe the Codex CONNECT proxy")?;
        if !timed_out && output.status.success() {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "Codex CONNECT proxy did not become ready: {}",
            cleanup_diagnostics(&output)
        );
        std::thread::sleep(CONTAINER_CLEANUP_RETRY_INTERVAL);
    }
}

fn wait_for_delayed_existence<F>(timeout: Duration, interval: Duration, mut exists: F) -> Result<()>
where
    F: FnMut() -> Result<bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if exists()? || Instant::now() >= deadline {
            return Ok(());
        }
        std::thread::sleep(interval);
    }
}

fn wait_for_delayed_volume(volume: &str) -> Result<()> {
    wait_for_delayed_existence(
        CONTAINER_MUTATION_SETTLE_TIMEOUT,
        CONTAINER_CLEANUP_RETRY_INTERVAL,
        || owned_volume_exists(volume),
    )
}

fn create_volume(resources: &CodexResources, volume: &str) -> Result<()> {
    let mut command = Command::new("docker");
    command.args(["volume", "create"]);
    resources.metadata().add_labels(&mut command, volume);
    command.arg(volume);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_STAGING_TIMEOUT)
        .context("could not create a private Codex volume")?;
    if timed_out {
        resources.mark_pending_mutation();
        wait_for_delayed_volume(volume)?;
        anyhow::bail!("Docker volume creation timed out");
    }
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "Docker volume creation failed: {}",
        cleanup_diagnostics(&output)
    );
    anyhow::ensure!(
        owned_volume_exists(volume)?,
        "private Codex volume disappeared after creation"
    );
    Ok(())
}

fn owned_volume_exists(volume: &str) -> Result<bool> {
    let mut command = Command::new("docker");
    command
        .args(["volume", "inspect", "--format"])
        .arg("{{.Name}}\t{{ index .Labels \"a3s.bench.codex.owner\" }}")
        .arg(volume);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not inspect Codex volume ownership")?;
    anyhow::ensure!(!timed_out, "Docker volume ownership inspection timed out");
    if !output.status.success() {
        if missing_volume(&output) {
            return Ok(false);
        }
        anyhow::bail!(
            "Docker volume ownership inspection failed: {}",
            cleanup_diagnostics(&output)
        );
    }
    anyhow::ensure!(
        !output.stdout_truncated && !output.stderr_truncated,
        "Docker volume ownership diagnostics were truncated"
    );
    let value =
        String::from_utf8(output.stdout).context("Docker volume ownership output was not UTF-8")?;
    let (name, owner) = value
        .trim()
        .split_once('\t')
        .ok_or_else(|| anyhow::anyhow!("Docker volume ownership output was malformed"))?;
    anyhow::ensure!(
        name == volume,
        "Docker returned an unexpected Codex volume name"
    );
    anyhow::ensure!(
        owner == volume,
        "refusing to operate on Codex volume {volume}: ownership label mismatch"
    );
    Ok(true)
}

fn remove_volume(volume: &str) -> Result<()> {
    if !owned_volume_exists(volume)? {
        return Ok(());
    }
    let mut command = Command::new("docker");
    command.args(["volume", "rm", volume]);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not run bounded Docker volume cleanup")?;
    let still_exists = owned_volume_exists(volume)?;
    if !still_exists {
        return Ok(());
    }
    anyhow::ensure!(!timed_out, "Docker volume cleanup timed out");
    anyhow::ensure!(
        output.status.success() && !output.stdout_truncated && !output.stderr_truncated,
        "Docker volume cleanup failed: {}",
        cleanup_diagnostics(&output)
    );
    anyhow::bail!("Docker volume still exists after cleanup")
}

fn missing_volume(output: &CommandOutput) -> bool {
    cleanup_diagnostics(output)
        .to_ascii_lowercase()
        .contains("no such volume")
}

fn owned_container_id(container: &str) -> Result<Option<String>> {
    let mut command = Command::new("docker");
    command
        .args(["container", "inspect", "--format"])
        .arg("{{.Id}}\t{{ index .Config.Labels \"a3s.bench.codex.owner\" }}")
        .arg(container);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not inspect Codex container ownership")?;
    anyhow::ensure!(
        !timed_out,
        "Docker container ownership inspection timed out"
    );
    if !output.status.success() {
        if missing_container(&output) {
            return Ok(None);
        }
        anyhow::bail!(
            "Docker container ownership inspection failed: {}",
            cleanup_diagnostics(&output)
        );
    }
    anyhow::ensure!(
        !output.stdout_truncated && !output.stderr_truncated,
        "Docker container ownership diagnostics were truncated"
    );
    let value = String::from_utf8(output.stdout)
        .context("Docker container ownership output was not UTF-8")?;
    let (id, owner) = value
        .trim()
        .split_once('\t')
        .ok_or_else(|| anyhow::anyhow!("Docker container ownership output was malformed"))?;
    anyhow::ensure!(!id.is_empty(), "Docker container id was empty");
    anyhow::ensure!(
        owner == container,
        "refusing to operate on Codex container {container}: ownership label mismatch"
    );
    Ok(Some(id.to_owned()))
}

fn wait_for_delayed_container(container: &str) -> Result<()> {
    let deadline = Instant::now() + CONTAINER_MUTATION_SETTLE_TIMEOUT;
    loop {
        if owned_container_id(container)?.is_some() || Instant::now() >= deadline {
            return Ok(());
        }
        std::thread::sleep(CONTAINER_CLEANUP_RETRY_INTERVAL);
    }
}

fn container_is_running(container_id: &str) -> Result<bool> {
    let mut command = Command::new("docker");
    command.args([
        "container",
        "inspect",
        "--format",
        "{{.State.Running}}",
        container_id,
    ]);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not inspect Codex container state")?;
    anyhow::ensure!(!timed_out, "Docker container state inspection timed out");
    if !output.status.success() {
        if missing_container(&output) {
            return Ok(false);
        }
        anyhow::bail!(
            "Docker container state inspection failed: {}",
            cleanup_diagnostics(&output)
        );
    }
    match String::from_utf8_lossy(&output.stdout).trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        value => anyhow::bail!("Docker returned invalid container state {value:?}"),
    }
}

fn missing_container(output: &CommandOutput) -> bool {
    let diagnostics = cleanup_diagnostics(output).to_ascii_lowercase();
    diagnostics.contains("no such object") || diagnostics.contains("no such container")
}

fn remove_container(container: &str) -> Result<()> {
    let Some(container_id) = owned_container_id(container)? else {
        return Ok(());
    };
    let mut command = Command::new("docker");
    command.args(["rm", "-f", "-v", &container_id]);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not run bounded Docker container cleanup")?;
    let still_exists = owned_container_id(container)?.is_some();
    if !still_exists {
        return Ok(());
    }
    anyhow::ensure!(!timed_out, "Docker container cleanup timed out");
    anyhow::ensure!(
        !output.stdout_truncated && !output.stderr_truncated,
        "Docker container cleanup diagnostics were truncated"
    );
    anyhow::ensure!(
        output.status.success(),
        "Docker container cleanup failed with {}: {}",
        output.status,
        cleanup_diagnostics(&output)
    );
    anyhow::bail!("Docker container still exists after cleanup")
}

fn all_resources_absent(resources: &CodexResources) -> Result<bool> {
    for container in [
        resources.main_container.as_str(),
        resources.proxy_container.as_deref().unwrap_or_default(),
        resources.staging_container.as_str(),
    ] {
        if !container.is_empty() && owned_container_id(container)?.is_some() {
            return Ok(false);
        }
    }
    if let Some(network) = resources.internal_network.as_deref() {
        if owned_internal_network_exists(network)? {
            return Ok(false);
        }
    }
    for volume in resources.volumes() {
        if owned_volume_exists(volume)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn remove_resources_once(resources: &CodexResources) -> Result<()> {
    let mut failures = Vec::new();
    let mut containers = vec![resources.main_container.as_str()];
    containers.extend(resources.proxy_container.as_deref());
    containers.push(resources.staging_container.as_str());
    for container in containers {
        if let Err(error) = remove_container(container) {
            failures.push(format!("{container}: {error:#}"));
        }
    }
    if let Some(network) = resources.internal_network.as_deref() {
        if let Err(error) = remove_internal_network(network) {
            failures.push(format!("{network}: {error:#}"));
        }
    }
    for volume in resources.volumes() {
        if let Err(error) = remove_volume(volume) {
            failures.push(format!("{volume}: {error:#}"));
        }
    }
    anyhow::ensure!(
        failures.is_empty(),
        "one or more Codex Docker resources could not be removed: {}",
        failures.join("; ")
    );
    Ok(())
}

fn retry_cleanup<F>(timeout: Duration, interval: Duration, mut cleanup: F) -> Result<()>
where
    F: FnMut() -> Result<()>,
{
    let deadline = Instant::now() + timeout;
    loop {
        match cleanup() {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(interval),
        }
    }
}

fn remove_resources(resources: &CodexResources) -> Result<()> {
    if !resources.has_pending_mutation() {
        return retry_cleanup(
            CONTAINER_CLEANUP_RETRY_TIMEOUT,
            CONTAINER_CLEANUP_RETRY_INTERVAL,
            || remove_resources_once(resources),
        );
    }

    let deadline = Instant::now() + CONTAINER_CLEANUP_RETRY_TIMEOUT;
    let mut stable = StableZeroWindow::new(PENDING_MUTATION_STABLE_ZERO);
    loop {
        let absent_before = all_resources_absent(resources).unwrap_or(false);
        let iteration_error = match remove_resources_once(resources) {
            Ok(()) => match all_resources_absent(resources) {
                Ok(absent_after) => {
                    if stable.observe(Instant::now(), absent_before && absent_after) {
                        return Ok(());
                    }
                    None
                }
                Err(error) => {
                    stable.observe(Instant::now(), false);
                    Some(error)
                }
            },
            Err(error) => {
                stable.observe(Instant::now(), false);
                Some(error)
            }
        };
        if Instant::now() >= deadline {
            return Err(iteration_error.unwrap_or_else(|| {
                anyhow::anyhow!("Codex Docker resources did not remain absent for five seconds")
            }));
        }
        std::thread::sleep(CONTAINER_CLEANUP_RETRY_INTERVAL);
    }
}

fn cleanup_diagnostics(output: &CommandOutput) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    match (stdout.trim(), stderr.trim()) {
        ("", "") => "Docker emitted no diagnostics".to_owned(),
        (stdout, "") => format!("stdout: {stdout}"),
        ("", stderr) => format!("stderr: {stderr}"),
        (stdout, stderr) => format!("stdout: {stdout}; stderr: {stderr}"),
    }
}

fn container_name() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "a3s-bench-codex-{}-{timestamp}-{}",
        std::process::id(),
        CODEX_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn persist_events(path: Option<&Path>, events: &[u8]) -> Result<()> {
    if let Some(path) = path {
        anyhow::ensure!(
            events.len() <= MAX_STDOUT_CAPTURE,
            "Codex event log exceeds the permitted size"
        );
        let value = String::from_utf8_lossy(events);
        crate::state_fs::secure_atomic_write(path, value.as_bytes())?;
    }
    Ok(())
}

fn redact_token_shaped_values(value: &str) -> String {
    let mut output = value.to_owned();
    for key in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "OPENAI_API_TOKEN",
        "ACCESS_TOKEN",
        "REFRESH_TOKEN",
    ] {
        for separator in ["=", ":"] {
            let marker = format!("{key}{separator}");
            let mut search_from = 0;
            while let Some(relative_start) = output[search_from..].find(&marker) {
                let start = search_from + relative_start;
                let value_start = start + marker.len();
                let end = output[value_start..]
                    .find(|character: char| {
                        character.is_whitespace()
                            || character == ','
                            || character == '}'
                            || character == '"'
                    })
                    .map(|offset| value_start + offset)
                    .unwrap_or(output.len());
                if end != value_start {
                    output.replace_range(value_start..end, "[redacted]");
                    search_from = value_start + "[redacted]".len();
                } else {
                    search_from = value_start;
                }
            }
        }
    }
    output
}

fn failure_diagnostics(events: &str, stderr: &str) -> String {
    let structured = events
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|event| {
            event
                .pointer("/error/message")
                .or_else(|| event.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .next_back();
    match (structured, stderr.trim()) {
        (Some(message), "") => redact_token_shaped_values(&message),
        (Some(message), stderr) => format!(
            "{}; stderr: {}",
            redact_token_shaped_values(&message),
            redact_token_shaped_values(stderr)
        ),
        (None, "") => "Codex emitted no diagnostics".to_string(),
        (None, stderr) => redact_token_shaped_values(stderr),
    }
}

fn parse_usage(events: &str) -> Result<Option<ModelExecution>> {
    let mut usage = None;
    let mut tool_calls_count = 0;
    for line in events.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value = serde_json::from_str(line).context("Codex emitted invalid JSONL")?;
        if event.get("type").and_then(Value::as_str) == Some("item.completed")
            && matches!(
                event.pointer("/item/type").and_then(Value::as_str),
                Some("command_execution" | "mcp_tool_call" | "file_change")
            )
        {
            tool_calls_count += 1;
        }
        let Some(value) = event.get("usage") else {
            continue;
        };
        let prompt_tokens = usize_field(value, "input_tokens")?;
        let completion_tokens = usize_field(value, "output_tokens")?;
        usage = Some(ModelExecution {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
            cache_read_tokens: optional_usize_field(value, "cached_input_tokens")?,
            cache_write_tokens: None,
            tool_calls_count,
        });
    }
    Ok(usage)
}

fn usize_field(value: &Value, name: &str) -> Result<usize> {
    optional_usize_field(value, name)?
        .ok_or_else(|| anyhow::anyhow!("Codex usage is missing {name}"))
}

fn optional_usize_field(value: &Value, name: &str) -> Result<Option<usize>> {
    value
        .get(name)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| anyhow::anyhow!("Codex usage {name} is invalid"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_structured_failure_before_stderr() {
        let events = concat!(
            r#"{"type":"error","message":"request failed"}"#,
            "\n",
            r#"{"type":"turn.failed","error":{"message":"model is unavailable"}}"#
        );
        assert_eq!(
            failure_diagnostics(events, "Reading additional input from stdin..."),
            "model is unavailable; stderr: Reading additional input from stdin..."
        );
    }

    #[test]
    fn parses_final_usage_and_counts_commands() {
        let events = concat!(
            r#"{"type":"item.completed","item":{"type":"command_execution"}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":12,"cached_input_tokens":3,"output_tokens":5}}"#
        );
        let usage = parse_usage(events).unwrap().unwrap();
        assert_eq!(usage.prompt_tokens, 12);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 17);
        assert_eq!(usage.cache_read_tokens, Some(3));
        assert_eq!(usage.tool_calls_count, 1);
    }

    #[test]
    fn diagnostics_redact_token_shaped_values() {
        assert_eq!(
            redact_token_shaped_values("OPENAI_API_KEY=sentinel, other"),
            "OPENAI_API_KEY=[redacted], other"
        );
        assert_eq!(
            redact_token_shaped_values("OPENAI_API_KEY=first OPENAI_API_KEY:second"),
            "OPENAI_API_KEY=[redacted] OPENAI_API_KEY:[redacted]"
        );
        assert_eq!(
            redact_token_shaped_values("OPENAI_API_KEY=, OPENAI_API_KEY:second"),
            "OPENAI_API_KEY=, OPENAI_API_KEY:[redacted]"
        );
    }

    #[test]
    fn codex_commands_use_edgebench_container() {
        let home = tempfile::tempdir().unwrap();
        let package_source = home.path().join("codex-package");
        let manifest = crate::codex_package::CodexPackageManifest {
            layout_version: 1,
            version: "1.0.0".into(),
            target_triple: "x86_64-unknown-linux-musl".into(),
            variant: "codex".into(),
            entrypoint: "bin/codex".into(),
            resources_dir: "codex-resources".into(),
            path_dir: "codex-path".into(),
        };
        for relative in [
            "bin/codex",
            "bin/codex-code-mode-host",
            "codex-path/rg",
            "codex-resources/bwrap",
        ] {
            let path = package_source.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    package_source.join(relative),
                    std::fs::Permissions::from_mode(0o700),
                )
                .unwrap();
            }
        }
        std::fs::write(
            package_source.join("codex-package.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let package =
            crate::codex_package::prepare_from_path(home.path(), &package_source, None).unwrap();
        let auth = home.path().join("auth.json");
        std::fs::write(&auth, "{}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let private_home =
            crate::codex_auth::stage_from_source(&home.path().join("state"), &auth).unwrap();
        let workspace = home.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let mut task = TaskInfo {
            id: "test".into(),
            name: "test".into(),
            category: "test".into(),
            judge_asset: "judge".into(),
            work_image: "alpine:3.20".into(),
            work_platform: Some("linux/amd64".into()),
            work_network_need: "none".into(),
            candidate_timeout_sec: 1,
            metrics: vec![],
            workspace_seed: Some(crate::task::WorkspaceSeed {
                image: "alpine:3.20".into(),
                source_path: "/app".into(),
                platform: Some("linux/amd64".into()),
            }),
            submission: crate::task::SubmissionPolicy {
                include: vec!["**".into()],
                exclude: vec![],
                max_files: 1,
                max_total_bytes: 1,
                max_file_bytes: 1,
            },
            legacy_judge: None,
            root: Path::new("/tmp/task").to_path_buf(),
        };
        let mut request = CodexExecutionRequest {
            task: &task,
            package: &package,
            workspace: &workspace,
            seed_workspace: None,
            instructions: "instructions",
            task_prompt: "task",
            model: Some("gpt-5.6-luna"),
            reasoning_effort: Some("none"),
            timeout_sec: 1,
            state_root: home.path(),
            event_log: None,
        };
        let resources = CodexResources::new("a3s-bench-codex-test".into(), true);
        let command = build_codex_run_command(&request, "prompt", &resources, "/app").unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.first().is_some_and(|arg| arg == "create"));
        assert!(args
            .windows(2)
            .any(|pair| { pair == ["--label", "a3s.bench.codex.owner=a3s-bench-codex-test"] }));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--entrypoint", "/opt/a3s/codex/bin/codex"]));
        assert!(args
            .iter()
            .any(|arg| arg == "CODEX_CODE_MODE_HOST_PATH=/opt/a3s/codex/bin/codex-code-mode-host"));
        // Omitting PATH preserves the Task image's Config.Env, including any
        // language toolchain prefixes installed by the image.
        assert!(!args.iter().any(|arg| arg.starts_with("PATH=")));
        let package_mount = format!(
            "type=volume,src={},dst={CONTAINER_PACKAGE},volume-subpath=codex,readonly",
            resources.package_volume
        );
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--mount", package_mount.as_str()]));
        assert_eq!(
            args.iter()
                .filter(|arg| arg.as_str() == package_mount)
                .count(),
            1
        );
        let home_mount = format!(
            "type=volume,src={},dst={CONTAINER_HOME},volume-subpath=home",
            resources.home_volume
        );
        assert!(private_home.codex_path().join("auth.json").is_file());
        assert!(!home_mount.ends_with(",readonly"));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--mount", home_mount.as_str()]));
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == home_mount).count(),
            1
        );
        assert!(!args.iter().any(|arg| arg.starts_with("type=bind")));
        assert!(!args.iter().any(|arg| {
            arg.contains(&package.root.display().to_string())
                || arg.contains(&private_home.path().display().to_string())
        }));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--env", "HOME=/run/a3s-codex/home"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--env", "CODEX_HOME=/run/a3s-codex/home/.codex"]));
        assert!(args
            .iter()
            .any(|arg| arg == "MAVEN_OPTS=-Dmaven.repo.local=/run/a3s-codex/home/.m2/repository"));
        let workspace_mount = format!("type=volume,src={},dst=/app", resources.workspace_volume);
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--mount", workspace_mount.as_str()]));
        assert!(!args
            .iter()
            .any(|arg| arg.contains(&workspace.display().to_string())));
        assert!(!args.iter().any(|arg| arg == "--user"));
        assert!(args
            .windows(2)
            .any(|pair| { pair == ["--tmpfs", "/run/a3s-codex:rw,noexec,nosuid,nodev,size=64m"] }));
        assert!(args
            .windows(2)
            .any(|pair| { pair == ["--network", resources.internal_network.as_deref().unwrap()] }));
        let proxy_url = format!(
            "HTTPS_PROXY=http://{}:{PROXY_PORT}",
            resources.proxy_container.as_deref().unwrap()
        );
        assert!(args.iter().any(|arg| arg == &proxy_url));
        assert!(args.contains(&"--ephemeral".into()));
        assert!(args.contains(&"--json".into()));
        assert!(args.contains(&"--skip-git-repo-check".into()));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["exec", "--dangerously-bypass-approvals-and-sandbox"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--model", "gpt-5.6-luna"]));
        let image_index = args.iter().position(|arg| arg == "alpine:3.20").unwrap();
        assert_eq!(
            &args[image_index + 1..image_index + 5],
            [
                "exec",
                "--dangerously-bypass-approvals-and-sandbox",
                "--cd",
                "/app"
            ]
        );
        let config_overrides = args
            .windows(2)
            .filter(|pair| pair[0] == "-c")
            .map(|pair| pair[1].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            config_overrides,
            [
                "shell_environment_policy.inherit=all",
                "shell_environment_policy.ignore_default_excludes=false",
                r#"shell_environment_policy.exclude=["HTTP_PROXY","HTTPS_PROXY","ALL_PROXY","NO_PROXY","http_proxy","https_proxy","all_proxy","no_proxy","CODEX_HOME","CODEX_CODE_MODE_HOST_PATH"]"#,
                "model_reasoning_effort=none",
            ]
        );
        assert_eq!(
            config_overrides
                .iter()
                .filter(|value| value.starts_with("shell_environment_policy.inherit="))
                .count(),
            1
        );
        assert!(!config_overrides.iter().any(|value| {
            *value == "shell_environment_policy.inherit=none"
                || value.starts_with("shell_environment_policy.include_only=")
                || value.starts_with("shell_environment_policy.set=")
        }));
        let separator_index = args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(args[separator_index + 1], "prompt");
        assert!(args.iter().any(|arg| arg == "model_reasoning_effort=none"));
        assert!(!args
            .iter()
            .any(|arg| arg == "--privileged" || arg == "danger-full-access"));
        assert!(!args.iter().any(|arg| {
            arg == "-s"
                || arg == "--sandbox"
                || arg == "workspace-write"
                || arg.contains("sandbox_workspace_write")
                || arg.contains("bwrap")
                || arg.contains("preflight")
                || arg.contains("API_KEY")
        }));

        request.seed_workspace = Some(&workspace);
        let seeded_resources = CodexResources::new("a3s-bench-codex-seeded-test".into(), true);
        let seeded =
            build_codex_run_command(&request, "prompt", &seeded_resources, "/app").unwrap();
        let seeded_args = seeded
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let seed_mount = format!(
            "type=volume,src={},dst=/app,volume-subpath=tree",
            seeded_resources.workspace_volume
        );
        assert!(seeded_args
            .windows(2)
            .any(|pair| pair == ["--mount", seed_mount.as_str()]));
        assert!(!seeded_args.iter().any(|arg| arg.starts_with("type=bind")));

        task.workspace_seed.as_mut().unwrap().source_path = "/home/workspace/".into();
        assert_eq!(container_workspace(&task).unwrap(), "/home/workspace");

        for invalid in [
            "/",
            "/run",
            "/run/",
            "//run",
            "/run//a3s-codex",
            "/a/./b",
            "/a/../b",
            "/app,readonly",
        ] {
            task.workspace_seed.as_mut().unwrap().source_path = invalid.into();
            assert!(container_workspace(&task).is_err(), "accepted {invalid:?}");
        }
        task.workspace_seed.as_mut().unwrap().source_path = "/app".into();
        task.root = home.path().join("task");
        std::fs::create_dir_all(task.root.join("public/workspace")).unwrap();
        assert_eq!(container_workspace(&task).unwrap(), "/workspace");
    }

    #[test]
    fn preserves_container_hardening_and_bounded_capture() {
        let mut command = Command::new("docker");
        let task = TaskInfo {
            id: "test".into(),
            name: "test".into(),
            category: "test".into(),
            judge_asset: "judge".into(),
            work_image: "alpine:3.20".into(),
            work_platform: None,
            work_network_need: "none".into(),
            candidate_timeout_sec: 1,
            metrics: vec![],
            workspace_seed: None,
            submission: crate::task::SubmissionPolicy {
                include: vec!["**".into()],
                exclude: vec![],
                max_files: 1,
                max_total_bytes: 1,
                max_file_bytes: 1,
            },
            legacy_judge: None,
            root: Path::new("/tmp/task").to_path_buf(),
        };
        let resources = CodexResources::new("a3s-bench-codex-test".into(), true);
        add_production_container_args(&mut command, &task, &resources, "/app", false).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|arg| arg == "--rm"));
        assert!(args.contains(&"--read-only".into()));
        assert!(args.first().is_some_and(|arg| arg == "create"));
        assert!(args.windows(2).any(|pair| pair == ["--cap-drop", "ALL"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--security-opt", "no-new-privileges"]));
        assert!(args.windows(2).any(|pair| pair == ["--pids-limit", "512"]));
        assert!(args.windows(2).any(|pair| pair == ["--memory", "8g"]));
        assert!(args.windows(2).any(|pair| pair == ["--cpus", "4"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--tmpfs", "/run/a3s-codex:rw,noexec,nosuid,nodev,size=64m"]));
        assert!(!args
            .iter()
            .any(|arg| arg.contains("empty-codex-home") || arg.contains("bwrap")));
        let workspace_mount = format!("type=volume,src={},dst=/app", resources.workspace_volume);
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--mount", workspace_mount.as_str()]));
        assert!(!args.iter().any(|arg| arg.starts_with("type=bind")));

        let mut public_task = task.clone();
        public_task.work_network_need = "public_internet".into();
        let public_resources = CodexResources::new("a3s-bench-codex-public-test".into(), false);
        let mut public_command = Command::new("docker");
        add_production_container_args(
            &mut public_command,
            &public_task,
            &public_resources,
            "/app",
            false,
        )
        .unwrap();
        add_proxy_environment(&mut public_command, &public_resources);
        let public_args = public_command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(public_args
            .windows(2)
            .any(|pair| pair == ["--network", "bridge"]));
        assert!(public_resources.proxy_container.is_none());
        assert!(public_resources.internal_network.is_none());
        assert!(public_resources.proxy_volume.is_none());
        assert!(!public_args
            .iter()
            .any(|arg| arg.contains("_PROXY=") || arg.contains("_proxy=")));

        let capture = drain_bounded(std::io::Cursor::new(b"012345"), 4).unwrap();
        assert_eq!(capture.bytes, b"0123");
        assert!(capture.truncated);
        let capture = drain_bounded(std::io::Cursor::new(b"0123"), 4).unwrap();
        assert_eq!(capture.bytes, b"0123");
        assert!(!capture.truncated);
    }
    #[test]
    fn proxy_sidecar_is_hardened_and_has_no_candidate_mounts() {
        let resources = CodexResources::new("a3s-bench-codex-proxy-test".into(), true);
        let command = build_proxy_create_command(&resources).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.first().is_some_and(|arg| arg == "create"));
        assert!(args.windows(2).any(|pair| pair == ["--pull", "never"]));
        assert!(args.contains(&"--read-only".into()));
        assert!(args.windows(2).any(|pair| pair == ["--cap-drop", "ALL"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--security-opt", "no-new-privileges"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--user", "65534:65534"]));
        assert!(args
            .windows(2)
            .any(|pair| { pair == ["--network", resources.internal_network.as_deref().unwrap()] }));
        assert!(!args.iter().any(|arg| arg == "bridge"));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--entrypoint", "python3"]));
        assert!(args.iter().any(|arg| arg == PROXY_HELPER_IMAGE));
        assert!(args.iter().any(|arg| arg == "--bind-internal"));
        assert!(!args.iter().any(|arg| arg == "0.0.0.0"));
        let bridge = build_proxy_bridge_connect_command("proxy-container-id");
        assert_eq!(
            bridge
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["network", "connect", "bridge", "proxy-container-id"]
        );
        assert!(!bridge
            .get_args()
            .any(|arg| arg.to_string_lossy() == "--gw-priority"));
        let mounts = args
            .windows(2)
            .filter_map(|pair| (pair[0] == "--mount").then_some(pair[1].as_str()))
            .collect::<Vec<_>>();
        assert_eq!(mounts.len(), 1);
        assert_eq!(
            mounts[0],
            format!(
                "type=volume,src={},dst={PROXY_CONTAINER_ROOT},readonly",
                resources.proxy_volume.as_deref().unwrap()
            )
        );
        for private in [
            &resources.package_volume,
            &resources.home_volume,
            &resources.workspace_volume,
        ] {
            assert!(!args.iter().any(|arg| arg.contains(private)));
        }
    }

    #[test]
    #[ignore = "requires Docker, the local python helper image, and public DNS/TLS"]
    fn docker_proxy_network_blocks_direct_and_enforces_connect_policy() {
        let root = tempfile::tempdir().unwrap();
        let resources = CodexResources::new(container_name(), true);
        let probe_container = format!("{}-probe", resources.main_container);
        let outcome = (|| -> Result<()> {
            ensure_proxy_helper_image()?;
            create_internal_network(&resources)?;
            for volume in resources.volumes() {
                create_volume(&resources, volume)?;
            }
            let asset = stage_proxy_runtime_asset(root.path())?;
            let proxy_volume = resources.proxy_volume.as_deref().unwrap();
            let mut stage = Command::new("docker");
            stage.args([
                "create",
                "--pull",
                "never",
                "--name",
                &resources.staging_container,
                "--label",
                &format!("{CONTAINER_OWNER_LABEL}={}", resources.staging_container),
                "--mount",
                &format!("type=volume,src={proxy_volume},dst={PROXY_STAGING_PARENT},volume-nocopy"),
                PROXY_HELPER_IMAGE,
                "/bin/true",
            ]);
            let (output, timed_out) = output_with_timeout(&mut stage, CONTAINER_STAGING_TIMEOUT)?;
            anyhow::ensure!(!timed_out, "Docker proxy test staging timed out");
            anyhow::ensure!(
                output.status.success(),
                "Docker proxy test staging failed: {}",
                cleanup_diagnostics(&output)
            );
            let staging_id = owned_container_id(&resources.staging_container)?
                .ok_or_else(|| anyhow::anyhow!("Docker proxy test staging disappeared"))?;
            copy_tree_into_container(
                &staging_id,
                &asset,
                &format!("{PROXY_STAGING_PARENT}/{PROXY_SCRIPT_NAME}"),
            )?;
            start_proxy_sidecar(&resources)?;
            let proxy_id = owned_container_id(resources.proxy_container.as_deref().unwrap())?
                .ok_or_else(|| anyhow::anyhow!("Docker proxy test sidecar disappeared"))?;
            anyhow::ensure!(
                container_has_network(&proxy_id, resources.internal_network.as_deref().unwrap())?,
                "Docker proxy test sidecar lost its internal network"
            );
            anyhow::ensure!(
                container_has_network(&proxy_id, "bridge")?,
                "Docker proxy test sidecar did not join bridge"
            );

            let script = r#"
import socket, ssl, sys
proxy = sys.argv[1]
try:
    direct = socket.create_connection(("1.1.1.1", 443), 2)
except OSError:
    pass
else:
    direct.close()
    raise SystemExit("direct public socket unexpectedly succeeded")

def connect(authority):
    sock = socket.create_connection((proxy, 3128), 3)
    request = (
        f"CONNECT {authority}:443 HTTP/1.1\r\n"
        f"Host: {authority}:443\r\n\r\n"
    ).encode("ascii")
    sock.sendall(request)
    response = b""
    while b"\r\n\r\n" not in response and len(response) < 4096:
        response += sock.recv(4096)
    return sock, response

denied, response = connect("example.com")
denied.close()
if not response.startswith(b"HTTP/1.1 403"):
    raise SystemExit(f"denied CONNECT returned {response[:64]!r}")

allowed, response = connect("api.openai.com")
if not response.startswith(b"HTTP/1.1 200"):
    raise SystemExit(f"allowed CONNECT returned {response[:64]!r}")
context = ssl._create_unverified_context()
tls = context.wrap_socket(allowed, server_hostname="api.openai.com")
tls.do_handshake()
tls.close()
print("ok")
"#;
            let mut probe = Command::new("docker");
            probe.args([
                "run",
                "--rm",
                "--pull",
                "never",
                "--name",
                &probe_container,
                "--label",
                &format!("{CONTAINER_OWNER_LABEL}={probe_container}"),
                "--network",
                resources.internal_network.as_deref().unwrap(),
                PROXY_HELPER_IMAGE,
                "python3",
                "-c",
                script,
                resources.proxy_container.as_deref().unwrap(),
            ]);
            let (output, timed_out) = output_with_timeout(&mut probe, Duration::from_secs(45))?;
            let probe_cleanup = remove_container(&probe_container);
            anyhow::ensure!(!timed_out, "Docker proxy policy probe timed out");
            anyhow::ensure!(
                output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "ok",
                "Docker proxy policy probe failed: {}",
                cleanup_diagnostics(&output)
            );
            probe_cleanup?;
            Ok(())
        })();
        let cleanup = remove_resources(&resources);
        outcome.unwrap();
        cleanup.unwrap();
    }

    #[test]
    #[ignore = "requires Docker and the local python helper image"]
    fn docker_stale_sweep_removes_inactive_group_and_preserves_active_group() {
        fn names(metadata: &RunMetadata) -> (String, String, String) {
            (
                format!("{}-stage", metadata.run_id),
                format!("{}-internal", metadata.run_id),
                format!("{}-home", metadata.run_id),
            )
        }

        fn create_group(metadata: &RunMetadata) -> Result<()> {
            let (container, network, volume) = names(metadata);
            let mut create = Command::new("docker");
            create.args(["create", "--pull", "never", "--name", &container]);
            metadata.add_labels(&mut create, &container);
            create.args([PROXY_HELPER_IMAGE, "/bin/true"]);
            let (output, timed_out) = output_with_timeout(&mut create, CONTAINER_STAGING_TIMEOUT)?;
            anyhow::ensure!(!timed_out, "stale sweep fixture container create timed out");
            anyhow::ensure!(
                output.status.success(),
                "stale sweep fixture container create failed: {}",
                cleanup_diagnostics(&output)
            );

            let mut create = Command::new("docker");
            create.args(["network", "create", "--internal"]);
            metadata.add_labels(&mut create, &network);
            create.arg(&network);
            let (output, timed_out) = output_with_timeout(&mut create, CONTAINER_STAGING_TIMEOUT)?;
            anyhow::ensure!(!timed_out, "stale sweep fixture network create timed out");
            anyhow::ensure!(
                output.status.success(),
                "stale sweep fixture network create failed: {}",
                cleanup_diagnostics(&output)
            );

            let mut create = Command::new("docker");
            create.args(["volume", "create"]);
            metadata.add_labels(&mut create, &volume);
            create.arg(&volume);
            let (output, timed_out) = output_with_timeout(&mut create, CONTAINER_STAGING_TIMEOUT)?;
            anyhow::ensure!(!timed_out, "stale sweep fixture volume create timed out");
            anyhow::ensure!(
                output.status.success(),
                "stale sweep fixture volume create failed: {}",
                cleanup_diagnostics(&output)
            );
            Ok(())
        }

        fn cleanup_group(metadata: &RunMetadata) -> Result<()> {
            let (container, network, volume) = names(metadata);
            let mut failures = Vec::new();
            if let Err(error) = remove_container(&container) {
                failures.push(format!("{container}: {error:#}"));
            }
            if let Err(error) = remove_internal_network(&network) {
                failures.push(format!("{network}: {error:#}"));
            }
            if let Err(error) = remove_volume(&volume) {
                failures.push(format!("{volume}: {error:#}"));
            }
            anyhow::ensure!(failures.is_empty(), "{}", failures.join("; "));
            Ok(())
        }

        ensure_proxy_helper_image().unwrap();
        let active = RunMetadata::current(container_name()).unwrap();
        let mut stale = RunMetadata::current(container_name()).unwrap();
        stale.boot_id.push_str("-previous-boot");
        let outcome = (|| -> Result<()> {
            create_group(&stale)?;
            create_group(&active)?;
            let sweeper = RunMetadata::current(container_name())?;
            sweep_stale_codex_resources(&sweeper)?;

            let (stale_container, stale_network, stale_volume) = names(&stale);
            anyhow::ensure!(
                owned_container_id(&stale_container)?.is_none()
                    && !owned_internal_network_exists(&stale_network)?
                    && !owned_volume_exists(&stale_volume)?,
                "inactive stale sweep fixture was not fully removed"
            );
            let (active_container, active_network, active_volume) = names(&active);
            anyhow::ensure!(
                owned_container_id(&active_container)?.is_some()
                    && owned_internal_network_exists(&active_network)?
                    && owned_volume_exists(&active_volume)?,
                "active stale sweep fixture was removed"
            );
            Ok(())
        })();
        let stale_cleanup = cleanup_group(&stale);
        let active_cleanup = cleanup_group(&active);
        outcome.unwrap();
        stale_cleanup.unwrap();
        active_cleanup.unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn materialized_workspace_is_writable_by_the_image_user() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let plain = nested.join("plain");
        let executable = nested.join("executable");
        std::fs::write(&plain, "plain").unwrap();
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

        prepare_workspace_for_container_copy(root.path()).unwrap();

        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(root.path()), 0o777);
        assert_eq!(mode(&nested), 0o777);
        assert_eq!(mode(&plain), 0o666);
        assert_eq!(mode(&executable), 0o777);
    }

    #[test]
    fn proc_start_ticks_parser_handles_spaces_in_command_name() {
        let stat = "123 (worker with spaces) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 4242";
        assert_eq!(parse_proc_stat_start_ticks(stat).unwrap(), 4242);
    }

    #[test]
    fn run_activity_requires_matching_boot_and_pid_start_ticks() {
        let metadata = RunMetadata {
            run_id: "run".into(),
            boot_id: "boot-a".into(),
            pid: 42,
            pid_start_ticks: 100,
            created_at: 1,
        };
        assert!(run_metadata_is_active(&metadata, "boot-a", |pid| {
            assert_eq!(pid, 42);
            Ok(Some(100))
        })
        .unwrap());
        assert!(!run_metadata_is_active(&metadata, "boot-a", |_| Ok(Some(101))).unwrap());
        assert!(!run_metadata_is_active(&metadata, "boot-a", |_| Ok(None)).unwrap());
        assert!(!run_metadata_is_active(&metadata, "boot-b", |_| {
            panic!("boot mismatch must not inspect a potentially reused pid")
        })
        .unwrap());
    }

    #[test]
    fn incomplete_run_metadata_is_rejected() {
        let mut labels = serde_json::Map::new();
        labels.insert(CONTAINER_RUN_LABEL.into(), Value::String("run".into()));
        labels.insert(CONTAINER_BOOT_ID_LABEL.into(), Value::String("boot".into()));
        labels.insert(CONTAINER_PID_LABEL.into(), Value::String("1".into()));
        labels.insert(CONTAINER_PID_START_LABEL.into(), Value::String("2".into()));
        let error = RunMetadata::from_labels("resource", &labels).unwrap_err();
        assert!(format!("{error:#}").contains(CONTAINER_CREATED_AT_LABEL));
    }

    #[test]
    fn resource_labels_include_complete_common_run_metadata() {
        let metadata = RunMetadata {
            run_id: "run-id".into(),
            boot_id: "boot-id".into(),
            pid: 7,
            pid_start_ticks: 11,
            created_at: 13,
        };
        let mut command = Command::new("docker");
        metadata.add_labels(&mut command, "resource-name");
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "--label",
                "a3s.bench.codex.owner=resource-name",
                "--label",
                "a3s.bench.codex.run=run-id",
                "--label",
                "a3s.bench.codex.boot_id=boot-id",
                "--label",
                "a3s.bench.codex.pid=7",
                "--label",
                "a3s.bench.codex.pid_start_ticks=11",
                "--label",
                "a3s.bench.codex.created_at=13",
            ]
        );
    }

    #[test]
    fn stable_zero_window_resets_when_a_resource_reappears() {
        let start = Instant::now();
        let mut stable = StableZeroWindow::new(Duration::from_secs(5));
        assert!(!stable.observe(start, true));
        assert!(!stable.observe(start + Duration::from_secs(4), true));
        assert!(!stable.observe(start + Duration::from_millis(4500), false));
        assert!(!stable.observe(start + Duration::from_secs(5), true));
        assert!(!stable.observe(start + Duration::from_secs(9), true));
        assert!(stable.observe(start + Duration::from_secs(10), true));
    }

    #[test]
    fn resource_inspect_missing_diagnostics_cover_all_docker_resource_kinds() {
        for (kind, diagnostics) in [
            (
                DockerResourceKind::Container,
                "Error: No such container: gone",
            ),
            (
                DockerResourceKind::Network,
                "Error response from daemon: network gone not found",
            ),
            (DockerResourceKind::Volume, "Error: No such volume: gone"),
        ] {
            assert!(resource_inspect_diagnostics_are_missing(kind, diagnostics));
            assert!(!resource_inspect_diagnostics_are_missing(
                kind,
                "permission denied"
            ));
        }
    }
    #[test]
    fn cleanup_retries_until_delayed_resources_disappear() {
        let mut attempts = 0;
        retry_cleanup(Duration::from_secs(1), Duration::ZERO, || {
            attempts += 1;
            if attempts < 3 {
                anyhow::bail!("resource still in use");
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(attempts, 3);
    }

    #[test]
    fn cleanup_returns_the_last_error_after_its_deadline() {
        let mut attempts = 0;
        let error = retry_cleanup(Duration::ZERO, Duration::ZERO, || {
            attempts += 1;
            anyhow::bail!("resource still in use")
        })
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert!(format!("{error:#}").contains("resource still in use"));
    }

    #[test]
    fn delayed_resource_wait_observes_absent_then_appearing_resource() {
        let mut attempts = 0;
        wait_for_delayed_existence(Duration::from_secs(1), Duration::ZERO, || {
            attempts += 1;
            Ok(attempts >= 3)
        })
        .unwrap();
        assert_eq!(attempts, 3);
    }

    #[test]
    fn run_guard_removes_private_auth_home_after_confirmed_removal() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let root = tempfile::tempdir().unwrap();
        let auth = root.path().join("auth.json");
        std::fs::write(&auth, "{}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let private_home =
            crate::codex_auth::stage_from_source(&root.path().join("state"), &auth).unwrap();
        let private_path = private_home.path().to_path_buf();
        let resources = CodexResources::new("a3s-bench-test-container".into(), true);
        let expected_resources = resources.clone();
        let order = Rc::new(RefCell::new(Vec::new()));
        let observed_path = private_path.clone();
        let remover_order = Rc::clone(&order);
        let guard = CodexRunGuard::with_remover(
            resources,
            private_home,
            Box::new(move |actual| {
                assert!(observed_path.join(".a3s-bench-codex-home").is_file());
                assert!(observed_path.join(".codex/auth.json").is_file());
                assert_eq!(actual.main_container, expected_resources.main_container);
                assert_eq!(
                    actual.staging_container,
                    expected_resources.staging_container
                );
                assert_eq!(actual.package_volume, expected_resources.package_volume);
                assert_eq!(actual.home_volume, expected_resources.home_volume);
                assert_eq!(actual.workspace_volume, expected_resources.workspace_volume);
                assert_eq!(actual.proxy_container, expected_resources.proxy_container);
                assert_eq!(actual.internal_network, expected_resources.internal_network);
                assert_eq!(actual.proxy_volume, expected_resources.proxy_volume);
                let names = [
                    actual.main_container.as_str(),
                    actual.proxy_container.as_deref().unwrap(),
                    actual.staging_container.as_str(),
                    actual.internal_network.as_deref().unwrap(),
                    actual.package_volume.as_str(),
                    actual.home_volume.as_str(),
                    actual.workspace_volume.as_str(),
                    actual.proxy_volume.as_deref().unwrap(),
                ];
                for (index, name) in names.iter().enumerate() {
                    assert!(!name.is_empty());
                    assert!(!names[..index].contains(name));
                }
                remover_order.borrow_mut().push("remove");
                Ok(())
            }),
        );
        guard.finish().unwrap();

        assert_eq!(&*order.borrow(), &["remove"]);
        assert!(!private_path.exists());
        assert!(!private_path.join(".codex/auth.json").exists());
    }

    #[test]
    fn run_guard_retains_private_auth_home_when_removal_fails() {
        let root = tempfile::tempdir().unwrap();
        let auth = root.path().join("auth.json");
        std::fs::write(&auth, "{}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let private_home =
            crate::codex_auth::stage_from_source(&root.path().join("state"), &auth).unwrap();
        let private_path = private_home.path().to_path_buf();
        let guard = CodexRunGuard::with_test_remover(
            "a3s-bench-test-container".into(),
            private_home,
            |_container| Err(anyhow::anyhow!("simulated container removal failure")),
        );
        drop(guard);
        assert!(private_path.exists());
        assert!(private_path.join(".a3s-bench-codex-home").is_file());
        assert!(private_path.join(".codex/auth.json").is_file());
    }
}
