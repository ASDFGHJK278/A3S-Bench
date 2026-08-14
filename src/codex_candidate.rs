use crate::codex_auth::PrivateCodexHome;
use crate::codex_package::CachedCodexPackage;
use crate::model_candidate::ModelExecution;
use crate::task::TaskInfo;
use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static CODEX_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const CONTAINER_HOME: &str = "/run/a3s-codex/home";
const CONTAINER_CODEX_HOME: &str = "/run/a3s-codex/home/.codex";
const CONTAINER_PACKAGE: &str = "/opt/a3s/codex";
const CONTAINER_WORKSPACE: &str = "/workspace";
const MINIMAL_CONTAINER_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
const MAX_STDOUT_CAPTURE: usize = 4 * 1024 * 1024;
const MAX_STDERR_CAPTURE: usize = 512 * 1024;
const CONTAINER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

pub enum CodexOutcome {
    Completed(Option<ModelExecution>),
    TimedOut,
}

pub struct CodexExecutionRequest<'a> {
    pub task: &'a TaskInfo,
    pub package: &'a CachedCodexPackage,
    pub workspace: &'a Path,
    pub instructions: &'a str,
    pub task_prompt: &'a str,
    pub model: Option<&'a str>,
    pub reasoning_effort: Option<&'a str>,
    pub timeout_sec: u64,
    pub state_root: &'a Path,
    pub event_log: Option<&'a Path>,
}

type ContainerRemover = Box<dyn FnMut(&str) -> Result<()> + 'static>;

struct CodexRunGuard {
    container: String,
    private_home: Option<PrivateCodexHome>,
    remover: ContainerRemover,
    container_removed: bool,
    finished: bool,
}

impl CodexRunGuard {
    fn new(container: String, private_home: PrivateCodexHome) -> Self {
        Self::with_remover(container, private_home, Box::new(remove_container))
    }

    fn with_remover(
        container: String,
        private_home: PrivateCodexHome,
        remover: ContainerRemover,
    ) -> Self {
        Self {
            container,
            private_home: Some(private_home),
            remover,
            container_removed: false,
            finished: false,
        }
    }

    #[cfg(test)]
    fn with_test_remover<F>(container: String, private_home: PrivateCodexHome, remover: F) -> Self
    where
        F: FnMut(&str) -> Result<()> + 'static,
    {
        Self::with_remover(container, private_home, Box::new(remover))
    }

    fn confirm_container_removed(&mut self) -> Result<()> {
        if self.container_removed {
            return Ok(());
        }
        (self.remover)(&self.container)
            .with_context(|| format!("could not remove Codex container {}", self.container))?;
        self.container_removed = true;
        Ok(())
    }

    fn cleanup_private_home(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.container_removed,
            "cannot clean up the private Codex home before container removal is confirmed"
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
                "warning: could not confirm removal of Codex container; retaining private Codex home for stale recovery: {error:#}"
            );
            return;
        }
        if let Err(error) = self.cleanup_private_home() {
            eprintln!(
                "warning: Codex container removal was confirmed, but private Codex home cleanup failed: {error:#}"
            );
        }
    }
}

pub fn execute(request: CodexExecutionRequest<'_>) -> Result<CodexOutcome> {
    crate::codex_package::validate_platform(
        request.package.target_triple(),
        request.task.work_platform.as_deref(),
    )?;

    let private_home = crate::codex_auth::stage(request.state_root, None)?;
    let container = container_name();
    let guard = CodexRunGuard::new(container.clone(), private_home);
    let prompt = format!(
        "{}\n\n# Benchmark task\n\n{}\n\nWork only in the supplied workspace and complete the task.",
        request.instructions, request.task_prompt
    );
    let result = (|| -> Result<CodexOutcome> {
        let mut command = build_codex_command(
            &request,
            guard
                .private_home
                .as_ref()
                .expect("Codex auth guard is live"),
            &prompt,
            &container,
        )?;
        let (output, timed_out) =
            output_with_timeout(&mut command, Duration::from_secs(request.timeout_sec))
                .context("could not start containerized Codex Candidate")?;
        let private_home = guard
            .private_home
            .as_ref()
            .expect("Codex auth guard is live");
        let events = private_home.redact(&output.stdout);
        let diagnostics = private_home.redact(&output.stderr);
        persist_events(request.event_log, &events)?;
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

fn build_codex_command(
    request: &CodexExecutionRequest<'_>,
    private_home: &PrivateCodexHome,
    prompt: &str,
    container: &str,
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
    add_production_container_args(&mut command, request.task, request.workspace, container)?;
    add_codex_environment(&mut command, request.package)?;
    request.package.verify_for_mount()?;
    add_codex_package_mounts(&mut command, request.package, request.workspace)?;
    validate_mount_path(private_home.codex_path())?;
    command
        .arg("--entrypoint")
        .arg(entrypoint)
        .arg("--mount")
        .arg(format!(
            "type=bind,src={},dst={CONTAINER_CODEX_HOME}",
            private_home.codex_path().display()
        ))
        .arg(&request.task.work_image)
        .args(codex_argv(request, prompt));
    Ok(command)
}

fn codex_argv(request: &CodexExecutionRequest<'_>, prompt: &str) -> Vec<String> {
    let mut argv = vec![
        "exec".into(),
        "--dangerously-bypass-approvals-and-sandbox".into(),
        "--cd".into(),
        CONTAINER_WORKSPACE.into(),
        "--ephemeral".into(),
        "--json".into(),
        "--skip-git-repo-check".into(),
        "--ignore-user-config".into(),
        "--ignore-rules".into(),
        "--color".into(),
        "never".into(),
        "-c".into(),
        "shell_environment_policy.inherit=none".into(),
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

fn container_directory(relative: &str) -> String {
    if relative.is_empty() {
        CONTAINER_PACKAGE.to_owned()
    } else {
        container_path(relative)
    }
}

fn add_codex_environment(command: &mut Command, package: &CachedCodexPackage) -> Result<()> {
    let paths = package.container_paths()?;
    let path = format!(
        "{}:{}:{MINIMAL_CONTAINER_PATH}",
        container_directory(&paths.entrypoint_dir),
        container_directory(&paths.path_dir),
    );
    command
        .arg("--env")
        .arg(format!("HOME={CONTAINER_HOME}"))
        .arg("--env");
    command.arg(format!("CODEX_HOME={CONTAINER_CODEX_HOME}"));
    command.arg("--env").arg(format!(
        "CODEX_CODE_MODE_HOST_PATH={}",
        container_path(&paths.code_mode_host)
    ));
    command.arg("--env").arg(format!("PATH={path}"));
    command.args(["--env", "LANG=C.UTF-8", "--env", "NO_COLOR=1"]);
    Ok(())
}

fn add_codex_package_mounts(
    command: &mut Command,
    package: &CachedCodexPackage,
    workspace: &Path,
) -> Result<()> {
    package.container_paths()?;
    validate_mount_path(&package.root)?;
    validate_mount_path(workspace)?;
    command.arg("--mount").arg(format!(
        "type=bind,src={},dst={CONTAINER_PACKAGE},readonly",
        package.root.display()
    ));
    // Docker's --mount parser is comma-delimited.  Keep the explicit
    // workspace mount next to the package mounts so all host paths pass the
    // same validation before command construction.
    command.arg("--mount").arg(format!(
        "type=bind,src={},dst={CONTAINER_WORKSPACE}",
        workspace.display()
    ));
    Ok(())
}

fn validate_mount_path(path: &Path) -> Result<()> {
    let value = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Docker mount path must be UTF-8"))?;
    anyhow::ensure!(
        !value.is_empty()
            && !value.contains(',')
            && !value.contains('\0')
            && !value.chars().any(char::is_control),
        "Docker mount path contains an unsafe character"
    );
    Ok(())
}

fn add_production_container_args(
    command: &mut Command,
    task: &TaskInfo,
    workspace: &Path,
    container: &str,
) -> Result<()> {
    add_container_args(command, task, workspace, container, "bridge")
}

fn add_container_args(
    command: &mut Command,
    task: &TaskInfo,
    workspace: &Path,
    container: &str,
    network: &str,
) -> Result<()> {
    validate_mount_path(workspace)?;
    command.args([
        "run",
        "--name",
        container,
        "--read-only",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
    ]);
    command.args(crate::runtime_profile::WORK_DOCKER_LIMITS);
    command.args([
        "--tmpfs",
        "/run/a3s-codex:rw,noexec,nosuid,nodev,size=64m",
        "--network",
        network,
    ]);
    if let Some(platform) = task.work_platform.as_deref() {
        command.args(["--platform", platform]);
    }
    configure_mounted_tree_owner(command, workspace)
}

fn configure_mounted_tree_owner(command: &mut Command, path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path)?;
        command.args(["--user", &format!("{}:{}", metadata.uid(), metadata.gid())]);
    }
    #[cfg(not(unix))]
    {
        let _ = (command, path);
    }
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
            child.kill()?;
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

fn remove_container(container: &str) -> Result<()> {
    let mut command = Command::new("docker");
    command.args(["rm", "-f", container]);
    let (output, timed_out) = output_with_timeout(&mut command, CONTAINER_CLEANUP_TIMEOUT)
        .context("could not run bounded Docker container cleanup")?;
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
    Ok(())
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
    format!(
        "a3s-bench-codex-{}-{}",
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
        let task = TaskInfo {
            id: "test".into(),
            name: "test".into(),
            category: "test".into(),
            judge_asset: "judge".into(),
            work_image: "alpine:3.20".into(),
            work_platform: Some("linux/amd64".into()),
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
        let request = CodexExecutionRequest {
            task: &task,
            package: &package,
            workspace: &workspace,
            instructions: "instructions",
            task_prompt: "task",
            model: Some("gpt-5.6-luna"),
            reasoning_effort: Some("none"),
            timeout_sec: 1,
            state_root: home.path(),
            event_log: None,
        };
        let command =
            build_codex_command(&request, &private_home, "prompt", "a3s-bench-codex-test").unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--entrypoint", "/opt/a3s/codex/bin/codex"]));
        assert!(args
            .iter()
            .any(|arg| arg == "CODEX_CODE_MODE_HOST_PATH=/opt/a3s/codex/bin/codex-code-mode-host"));
        assert!(args.iter().any(|arg| {
            arg == "PATH=/opt/a3s/codex/bin:/opt/a3s/codex/codex-path:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
        }));
        let package_mount = format!(
            "type=bind,src={},dst={CONTAINER_PACKAGE},readonly",
            package.root.display()
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
        let codex_mount = format!(
            "type=bind,src={},dst={CONTAINER_CODEX_HOME}",
            private_home.codex_path().display()
        );
        assert!(private_home.codex_path().join("auth.json").is_file());
        assert!(!codex_mount.ends_with(",readonly"));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--mount", codex_mount.as_str()]));
        assert_eq!(
            args.iter()
                .filter(|arg| arg.as_str() == codex_mount)
                .count(),
            1
        );
        let private_root_mount = format!("type=bind,src={},", private_home.path().display());
        assert!(!args.iter().any(|arg| arg.starts_with(&private_root_mount)));
        let private_home_mount = format!(
            "type=bind,src={},dst={CONTAINER_HOME}",
            private_home.path().display()
        );
        assert!(!args.iter().any(|arg| arg == &private_home_mount));
        let marker = private_home.path().join(".a3s-bench-codex-home");
        let marker = marker.to_string_lossy();
        assert!(!args.iter().any(|arg| arg.contains(marker.as_ref())));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--env", "HOME=/run/a3s-codex/home"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--env", "CODEX_HOME=/run/a3s-codex/home/.codex"]));
        let workspace_mount = format!(
            "type=bind,src={},dst={CONTAINER_WORKSPACE}",
            workspace.display()
        );
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--mount", workspace_mount.as_str()]));
        assert!(args.windows(2).any(|pair| pair == ["--network", "bridge"]));
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
                CONTAINER_WORKSPACE
            ]
        );
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
    }

    #[test]
    fn preserves_container_hardening_and_bounded_capture() {
        let workspace = tempfile::tempdir().unwrap();
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
        add_production_container_args(
            &mut command,
            &task,
            workspace.path(),
            "a3s-bench-codex-test",
        )
        .unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(!args.iter().any(|arg| arg == "--rm"));
        assert!(args.contains(&"--read-only".into()));
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

        let capture = drain_bounded(std::io::Cursor::new(b"012345"), 4).unwrap();
        assert_eq!(capture.bytes, b"0123");
        assert!(capture.truncated);
        let capture = drain_bounded(std::io::Cursor::new(b"0123"), 4).unwrap();
        assert_eq!(capture.bytes, b"0123");
        assert!(!capture.truncated);
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
        let order = Rc::new(RefCell::new(Vec::new()));
        let observed_path = private_path.clone();
        let remover_order = Rc::clone(&order);
        let guard = CodexRunGuard::with_test_remover(
            "a3s-bench-test-container".into(),
            private_home,
            move |_container| {
                assert!(observed_path.join(".a3s-bench-codex-home").is_file());
                assert!(observed_path.join(".codex/auth.json").is_file());
                remover_order.borrow_mut().push("remove");
                Ok(())
            },
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
