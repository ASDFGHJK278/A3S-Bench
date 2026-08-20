#!/bin/sh
set -u

if [ "$#" -ne 5 ]; then
    echo "Codex run loop received an invalid argument vector" >&2
    exit 2
fi

codex="$1"
model="$2"
reasoning_effort="$3"
disable_web_search="$4"
prompt="$5"
max_resumes=100
min_runtime_for_resume_centiseconds=100
resume_count=0

monotonic_centiseconds() {
    IFS=' ' read -r uptime_seconds _ </proc/uptime
    whole_seconds=${uptime_seconds%%.*}
    fractional_seconds=${uptime_seconds#*.}
    fractional_seconds=$(printf '%.2s' "${fractional_seconds}00")
    printf '%s\n' "$((whole_seconds * 100 + 1$fractional_seconds - 100))"
}

run_initial() {
    set -- exec
    if [ "$disable_web_search" = 1 ]; then
        set -- "$@" -c 'web_search="disabled"'
    fi
    set -- "$@" --dangerously-bypass-approvals-and-sandbox --json --skip-git-repo-check -c features.hooks=true
    if [ "$model" = gpt-5.3-codex-spark ]; then
        set -- "$@" --disable image_generation -c model_reasoning_summary=none
    fi
    if [ -n "$model" ]; then
        set -- "$@" --model "$model"
    fi
    if [ -n "$reasoning_effort" ]; then
        set -- "$@" -c "model_reasoning_effort=$reasoning_effort"
    fi
    set -- "$@" -- "$prompt"
    "$codex" "$@"
}

run_resume() {
    set -- exec
    if [ "$disable_web_search" = 1 ]; then
        set -- "$@" -c 'web_search="disabled"'
    fi
    set -- "$@" resume --last --dangerously-bypass-approvals-and-sandbox --json --skip-git-repo-check -c features.hooks=true
    if [ "$model" = gpt-5.3-codex-spark ]; then
        set -- "$@" --disable image_generation -c model_reasoning_summary=none
    fi
    if [ -n "$model" ]; then
        set -- "$@" --model "$model"
    fi
    if [ -n "$reasoning_effort" ]; then
        set -- "$@" -c "model_reasoning_effort=$reasoning_effort"
    fi
    set -- "$@" 'Continue working.'
    "$codex" "$@"
}

started=$(monotonic_centiseconds)
run_initial
status=$?
elapsed=$(($(monotonic_centiseconds) - started))

while [ "$elapsed" -ge "$min_runtime_for_resume_centiseconds" ] && [ "$resume_count" -lt "$max_resumes" ]; do
    resume_count=$((resume_count + 1))
    printf 'Resuming Codex (attempt %d/%d)\n' "$resume_count" "$max_resumes" >&2
    started=$(monotonic_centiseconds)
    run_resume
    status=$?
    elapsed=$(($(monotonic_centiseconds) - started))
done

if [ "$elapsed" -lt "$min_runtime_for_resume_centiseconds" ]; then
    printf 'Codex exited after %dms; not resuming a likely systematic failure\n' "$((elapsed * 10))" >&2
elif [ "$resume_count" -ge "$max_resumes" ]; then
    printf 'Codex reached the maximum of %d resume attempts\n' "$max_resumes" >&2
fi

exit "$status"