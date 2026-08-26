use crate::task::RoleResources;

pub fn work_docker_args(resources: RoleResources) -> Vec<String> {
    vec![
        "--pids-limit".into(),
        "512".into(),
        "--memory".into(),
        resources.memory_bytes.to_string(),
        "--cpus".into(),
        resources.cpu_limit.to_string(),
    ]
}

pub fn judge_docker_args(resources: RoleResources) -> Vec<String> {
    vec![
        "--memory".into(),
        resources.memory_bytes.to_string(),
        "--cpus".into(),
        resources.cpu_limit.to_string(),
    ]
}

pub const READ_ONLY_JUDGE_TMPFS: &[&str] = &[];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_profiles_apply_locked_resources_without_duplicate_limits() {
        let work = work_docker_args(RoleResources {
            cpu_limit: 6,
            memory_bytes: 12_345,
        });
        assert!(work.windows(2).any(|pair| pair == ["--memory", "12345"]));
        assert!(work.windows(2).any(|pair| pair == ["--cpus", "6"]));
        assert!(!work.iter().any(|value| value == "--tmpfs"));

        let judge = judge_docker_args(RoleResources {
            cpu_limit: 3,
            memory_bytes: 54_321,
        });
        assert!(judge.windows(2).any(|pair| pair == ["--memory", "54321"]));
        assert!(judge.windows(2).any(|pair| pair == ["--cpus", "3"]));
        assert!(!judge.iter().any(|value| value == "--tmpfs"));
        assert!(READ_ONLY_JUDGE_TMPFS.is_empty());
    }
}
