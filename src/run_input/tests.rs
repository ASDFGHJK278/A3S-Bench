use super::*;

#[test]
fn rejects_duplicate_run_options() {
    let args = vec![
        "./task".into(),
        "--agent".into(),
        "./agent".into(),
        "--json".into(),
        "--json".into(),
    ];
    assert!(RunOptions::parse(&args).is_err());
}

#[test]
fn parses_codex_model_and_reasoning_effort() {
    let args = vec![
        "./task".into(),
        "--agent".into(),
        "./agent".into(),
        "--codex-model".into(),
        "o3".into(),
        "--codex-reasoning-effort".into(),
        "high".into(),
    ];
    let opts = RunOptions::parse(&args).unwrap();
    assert_eq!(opts.codex_model.as_deref(), Some("o3"));
    assert_eq!(opts.codex_reasoning_effort.as_deref(), Some("high"));
}

#[test]
fn codex_options_are_optional() {
    let args = vec!["./task".into(), "--agent".into(), "./agent".into()];
    let opts = RunOptions::parse(&args).unwrap();
    assert!(opts.codex_model.is_none());
    assert!(opts.codex_reasoning_effort.is_none());
}

#[test]
fn rejects_duplicate_codex_model() {
    let args = vec![
        "./task".into(),
        "--agent".into(),
        "./agent".into(),
        "--codex-model".into(),
        "o3".into(),
        "--codex-model".into(),
        "gpt-4.1".into(),
    ];
    assert!(RunOptions::parse(&args).is_err());
}
