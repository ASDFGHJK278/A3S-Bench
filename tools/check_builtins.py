#!/usr/bin/env python3
"""Offline structural and provenance checks for the global builtin catalog."""

from __future__ import annotations

import hashlib
import json
import re
from collections import Counter
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1] / "builtin"
PROVENANCE_PATH = ROOT / "provenance" / "edgebench.json"
EXPECTED_DATASET_COMMIT = "47846a4c3669ad447e0ea984833b0d352460c5f9"
EXPECTED_HARNESS_COMMIT = "f59bcb0f024d4bc8baedeac271306050e4bb0d33"
EXPECTED_EXPERIMENT_PATH = "examples/all-tasks-k8s/experiment-codex.yaml"
EXPECTED_EXPERIMENT_SHA256 = (
    "sha256:6cf7d41bc67f765adbd458409209e6af02bbb9a53020cf366511707b3fc45b33"
)
EXPECTED_TASKS = 51
EXPECTED_MODES = {"batch": 48, "game_server": 3}
EXPECTED_BLOCKED_TASKS = {}
EXPECTED_RESOURCE_PROFILES = {
    (4, 16 * 1024**3, 4, 8 * 1024**3): 42,
    (4, 16 * 1024**3, 4, 16 * 1024**3): 1,
    (8, 16 * 1024**3, 8, 16 * 1024**3): 7,
    (16, 16 * 1024**3, 16, 16 * 1024**3): 1,
}
EXPECTED_WORKSPACE_IMPORTS = {
    "exchange_core_throughput": [
        {
            "name": "maven_repository",
            "source_path": "/root/.m2/repository",
            "target_path": "/home/agent/.m2/repository",
        }
    ]
}
PYPI_ALLOW_HOSTS = ["files.pythonhosted.org", "pypi.org"]
EXPECTED_NETWORK_ALLOW_HOSTS = {
    "cta_risk_budget_optimization": PYPI_ALLOW_HOSTS,
    "k12_math_recommendation": PYPI_ALLOW_HOSTS,
    "schemathesis_config_modernization": PYPI_ALLOW_HOSTS,
    "schemathesis_datagen_pipeline": PYPI_ALLOW_HOSTS,
    "schemathesis_reporting_observability": PYPI_ALLOW_HOSTS,
    "exchange_core_throughput": ["repo.maven.apache.org"],
    "new_foundations_consistency": ["github.com"],
}
ORDER_ADDITION_OLD_HELPER_SHA256 = (
    "3023b9a449119e862d4ca86d3ab45599e2496e182be82959a642522f915dbbac"
)
ORDER_ADDITION_ACTUAL_HELPER_SHA256 = (
    "337837af7067b3dae8d4ef068d26d8dd8ff779f9a627d95451af2ca411c99630"
)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def acl_block(source: str, name: str) -> str:
    match = re.search(rf"^\s*{re.escape(name)}\s*\{{\s*$", source, re.MULTILINE)
    if match is None:
        raise AssertionError(f"missing ACL block: {name}")
    start = match.start()
    depth = 0
    for index in range(match.start(), len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start : index + 1]
    raise AssertionError(f"unterminated ACL block: {name}")


def require_acl_integer(block: str, field: str, expected: int, task_id: str) -> None:
    values = re.findall(
        rf"^\s*{re.escape(field)}\s*=\s*([0-9]+)\s*$", block, re.MULTILINE
    )
    require(values == [str(expected)], f"{field}: {task_id}")


def main() -> None:
    catalog = json.loads((ROOT / "catalog.json").read_text(encoding="utf-8"))
    provenance = json.loads(PROVENANCE_PATH.read_text(encoding="utf-8"))
    require(catalog["schema"] == "a3s-bench/builtin-catalog/v1", "catalog schema")
    require(set(catalog) == {"schema", "tasks"}, "catalog fields")
    require(provenance["dataset"]["commit"] == EXPECTED_DATASET_COMMIT, "dataset commit")
    require(provenance["harness"]["commit"] == EXPECTED_HARNESS_COMMIT, "harness commit")
    require(
        provenance["harness"]["experiment"]
        == {
            "path": EXPECTED_EXPERIMENT_PATH,
            "sha256": EXPECTED_EXPERIMENT_SHA256,
        },
        "pinned harness experiment",
    )
    require(provenance["task_count"] == EXPECTED_TASKS, "provenance task count")
    require((ROOT / "licenses" / "CC-BY-4.0.txt").is_file(), "CC BY license")
    require((ROOT / "licenses" / "Apache-2.0.txt").is_file(), "Apache license")
    require((ROOT / "README.md").is_file(), "builtin README")
    require((ROOT / "THIRD_PARTY_NOTICES.md").is_file(), "third-party notices")

    records = {record["task_id"]: record for record in provenance["records"]}
    entries = [
        entry
        for entry in catalog["tasks"]
        if entry["provenance_ref"].startswith("provenance/edgebench.json#")
    ]
    require(len(entries) == EXPECTED_TASKS, "imported catalog task count")
    ids = [entry["id"] for entry in entries]
    require(ids == sorted(ids), "catalog ordering")
    require(len(ids) == len(set(ids)), "duplicate task id")
    require(set(records) == set(ids), "provenance records")

    modes: Counter[str] = Counter()
    resource_profiles: Counter[tuple[int, int, int, int]] = Counter()
    model_gateway_count = 0
    for entry in entries:
        task_id = entry["id"]
        require(
            set(entry)
            == {
                "id",
                "path",
                "name",
                "category",
                "execution_class",
                "availability",
                "availability_reason",
                "admission",
                "admission_reason",
                "provenance_ref",
            },
            f"discovery-only catalog entry: {task_id}",
        )
        require(re.fullmatch(r"[a-z][a-z0-9_]{0,63}", task_id) is not None, f"task id: {task_id}")
        task_root = ROOT / "tasks" / task_id
        task_acl_path = task_root / "task.acl"
        prompt_path = task_root / "public" / "prompt.md"
        private_root = task_root / "private"
        asset_root = private_root / "judge"
        bundle_root = private_root / "bundle"
        asset_acl_path = asset_root / ".a3s" / "asset.acl"
        agent_path = asset_root / "agent.md"
        descriptor_path = asset_root / "judge.source.json"
        generated = {
            "task.acl": task_acl_path,
            "public/prompt.md": prompt_path,
            "private/judge/.a3s/asset.acl": asset_acl_path,
            "private/judge/agent.md": agent_path,
            "private/judge/judge.source.json": descriptor_path,
        }
        for path in generated.values():
            require(path.is_file(), f"missing {path.relative_to(ROOT)}")
            require(not path.is_symlink(), f"symlink forbidden: {path.relative_to(ROOT)}")
        require(not bundle_root.exists(), f"unavailable hidden bundle must be absent: {task_id}")
        require(
            {path.name for path in private_root.iterdir()} == {"judge"},
            f"canonical private layout: {task_id}",
        )

        record = records[task_id]
        require(re.fullmatch(r"sha256:[0-9a-f]{64}", record["source_sha256"]) is not None, f"source digest: {task_id}")
        require(record["modified"] is True, f"adaptation flag: {task_id}")
        resources = record["resolved_resources"]
        require(set(resources) == {"work", "judge"}, f"resource roles: {task_id}")
        for role in ("work", "judge"):
            require(
                set(resources[role]) == {"cpu_limit", "memory_bytes"},
                f"resource fields: {task_id}/{role}",
            )
            require(
                type(resources[role]["cpu_limit"]) is int
                and resources[role]["cpu_limit"] > 0,
                f"CPU resource: {task_id}/{role}",
            )
            require(
                type(resources[role]["memory_bytes"]) is int
                and resources[role]["memory_bytes"]
                in {8 * 1024**3, 16 * 1024**3},
                f"memory resource: {task_id}/{role}",
            )
        resource_profiles[
            (
                resources["work"]["cpu_limit"],
                resources["work"]["memory_bytes"],
                resources["judge"]["cpu_limit"],
                resources["judge"]["memory_bytes"],
            )
        ] += 1
        expected_imports = EXPECTED_WORKSPACE_IMPORTS.get(task_id, [])
        require(
            record.get("workspace_imports", []) == expected_imports,
            f"workspace import provenance: {task_id}",
        )
        expected_network_hosts = EXPECTED_NETWORK_ALLOW_HOSTS.get(task_id, [])
        network_adaptation = record.get("network_adaptation")
        if expected_network_hosts:
            require(
                network_adaptation is not None
                and network_adaptation["network_need"] == "restricted_https"
                and network_adaptation["https_allow_hosts"] == expected_network_hosts
                and isinstance(network_adaptation["reason"], str)
                and network_adaptation["reason"],
                f"network adaptation provenance: {task_id}",
            )
        else:
            require(network_adaptation is None, f"unexpected network adaptation: {task_id}")
        for relative, path in generated.items():
            require(
                record["generated_sha256"][relative] == f"sha256:{sha256(path)}",
                f"generated digest: {task_id}/{relative}",
            )

        task_acl = task_acl_path.read_text(encoding="utf-8")
        asset_acl = asset_acl_path.read_text(encoding="utf-8")
        agent_md = agent_path.read_text(encoding="utf-8")
        descriptor = json.loads(descriptor_path.read_text(encoding="utf-8"))
        require(entry["path"] == f"tasks/{task_id}", f"path: {task_id}")
        require(entry["execution_class"] == "long_horizon", f"execution class: {task_id}")
        expected_availability = "blocked" if task_id in EXPECTED_BLOCKED_TASKS else "ready"
        require(entry["availability"] == expected_availability, f"availability: {task_id}")
        expected_availability_reason = EXPECTED_BLOCKED_TASKS.get(
            task_id,
            "requires_configured_judge_model"
            if task_id == "college_english_exam_bank"
            else "bundled_oci_task",
        )
        require(
            entry["availability_reason"] == expected_availability_reason,
            f"availability reason: {task_id}",
        )
        require(entry["admission"] == "quarantined", f"catalog admission: {task_id}")
        require(
            entry["admission_reason"] == "official_evidence_incomplete",
            f"catalog admission reason: {task_id}",
        )
        require(task_acl.count("{") == task_acl.count("}"), f"ACL braces: {task_id}")
        require(task_acl.count("[") == task_acl.count("]"), f"ACL arrays: {task_id}")
        require(asset_acl.count("{") == asset_acl.count("}"), f"asset braces: {task_id}")
        require("dev_visible" not in task_acl, f"legacy dev visibility field: {task_id}")
        require(re.search(rf'^bench "{re.escape(task_id)}" \{{$', task_acl, re.MULTILINE) is not None, f"ACL id: {task_id}")
        require('schema  = "a3s-bench/task/v2"' in task_acl, f"ACL schema: {task_id}")
        work_acl = acl_block(task_acl, "work")
        judge_acl = acl_block(task_acl, "judge")
        parsed_imports = [
            {"name": name, "source_path": source_path, "target_path": target_path}
            for name, source_path, target_path in re.findall(
                r'workspace_import\s+"([^"]+)"\s*\{\s*'
                r'source_path\s*=\s*"([^"]+)"\s*'
                r'target_path\s*=\s*"([^"]+)"\s*\}',
                work_acl,
            )
        ]
        require(
            work_acl.count("workspace_import") == len(expected_imports)
            and parsed_imports == expected_imports,
            f"workspace imports: {task_id}",
        )
        allow_match = re.search(
            r"^\s*https_allow_hosts\s*=\s*\[(.*?)\]",
            work_acl,
            re.MULTILINE | re.DOTALL,
        )
        parsed_network_hosts = (
            re.findall(r'"([a-z0-9.-]+)"', allow_match.group(1))
            if allow_match
            else []
        )
        require(
            parsed_network_hosts == expected_network_hosts,
            f"HTTPS allow hosts: {task_id}",
        )
        network_need = re.findall(
            r'^\s*network_need\s*=\s*"([a-z_]+)"\s*$',
            work_acl,
            re.MULTILINE,
        )
        require(len(network_need) == 1, f"network need: {task_id}")
        if expected_network_hosts:
            require(network_need == ["restricted_https"], f"restricted network: {task_id}")
        require_acl_integer(
            work_acl, "cpu_limit", resources["work"]["cpu_limit"], task_id
        )
        require_acl_integer(
            work_acl, "memory_bytes", resources["work"]["memory_bytes"], task_id
        )
        require_acl_integer(
            judge_acl, "cpu_limit", resources["judge"]["cpu_limit"], task_id
        )
        require_acl_integer(
            judge_acl, "memory_bytes", resources["judge"]["memory_bytes"], task_id
        )
        require('metric "score"' in task_acl, f"native metric: {task_id}")
        require(re.search(r'^\s*asset\s*=\s*"private/judge"$', task_acl, re.MULTILINE) is not None, f"task Judge ref: {task_id}")
        require('version = "a3s.asset.v1"' in asset_acl, f"asset version: {task_id}")
        require('category = "agent"' in asset_acl, f"asset category: {task_id}")
        require('capability "bench.judge.v1"' in asset_acl, f"Judge capability: {task_id}")
        require('input_schema  = "bench.judge.request.v1"' in asset_acl, f"Judge input: {task_id}")
        require('output_schema = "bench.judge.result.v1"' in asset_acl, f"Judge output: {task_id}")
        require("benchmark {" not in asset_acl, f"private Judge dialect: {task_id}")
        expected_gateway = "scoped" if "model_gateway" in descriptor["requirements"] else "none"
        require(
            f'model_gateway = "{expected_gateway}"' in asset_acl,
            f"Judge ModelGateway capability: {task_id}",
        )
        require("EdgeBench" not in asset_acl, f"source-specific asset name: {task_id}")
        require("EdgeBench" not in agent_md, f"source-specific agent content: {task_id}")
        require("Bench runner" not in agent_md, f"private Bench runner wording: {task_id}")
        require(descriptor["schema"] == "a3s-bench/judge-source/v1", f"descriptor schema: {task_id}")
        require(descriptor["admission"] == "quarantined", f"descriptor admission: {task_id}")
        require(descriptor["kind"] == "oci", f"descriptor kind: {task_id}")
        require(descriptor["image"]["platform"] == "linux/amd64", f"platform: {task_id}")
        require("upstream" not in descriptor, f"duplicated provenance: {task_id}")
        if task_id == "order_addition_permutation_optimization":
            source_command = descriptor["evaluation"]["source_command"]
            require(
                source_command.startswith(
                    "cd /home/workspace/complex_job_scheduling && python -c "
                ),
                "Order Addition Judge working directory",
            )
            require(
                source_command.endswith(
                    "&& python -m pytest tests/test_final_result.py -s -v"
                ),
                "Order Addition original pytest command",
            )
            require(
                ORDER_ADDITION_OLD_HELPER_SHA256 in source_command
                and ORDER_ADDITION_ACTUAL_HELPER_SHA256 in source_command,
                "Order Addition helper hash adaptation",
            )
        if task_id == "cta_risk_budget_optimization":
            source_command = descriptor["evaluation"]["source_command"]
            for required in (
                "--target /tmp/a3s-judge-deps-cta",
                "pandas==1.5.3",
                "numpy==1.23.5",
                "scipy==1.10.1",
                "scikit-learn==1.2.2",
                "statsmodels==0.13.5",
                "matplotlib==3.7.1",
                "openpyxl==3.1.2",
                "PYTHONPATH=/tmp/a3s-judge-deps-cta",
                "|| exit 125;",
            ):
                require(required in source_command, f"CTA Judge dependency adapter: {required}")
            require("requirements.txt" not in source_command, "CTA Judge trusts submission requirements")
        if task_id == "k12_math_recommendation":
            source_command = descriptor["evaluation"]["source_command"]
            require(
                "--no-deps --target /tmp/a3s-judge-deps-k12" in source_command
                and "networkx==3.3" in source_command
                and "PYTHONPATH=/tmp/a3s-judge-deps-k12" in source_command,
                "K12 Judge dependency adapter",
            )
            require("|| exit 125;" in source_command, "K12 Judge install is not fail-closed")
            require("requirements.txt" not in source_command, "K12 Judge trusts submission requirements")
        modes[descriptor["evaluation"]["mode"]] += 1
        model_gateway_count += int("model_gateway" in descriptor["requirements"])

    require(dict(modes) == EXPECTED_MODES, "Judge modes")
    require(dict(resource_profiles) == EXPECTED_RESOURCE_PROFILES, "resource profiles")
    require(model_gateway_count == 1, "model-gateway Judge count")
    forbidden = [path for path in ROOT.rglob("*") if ".a3s-bench" in path.parts]
    require(not forbidden, "forbidden .a3s-bench path")
    require(not (ROOT / "edgebench").exists(), "source-specific builtin root")
    require(not (ROOT / "upstream").exists(), "upstream mirror directory")
    require(not list((ROOT / "tasks").glob("*/upstream.json")), "copied source records")
    legacy_layouts = [
        path
        for task_root in (ROOT / "tasks").iterdir()
        for path in (
            task_root / "private" / "judge" / "asset",
            task_root / "private" / "judge-dev",
            task_root / "private" / "judge-final",
        )
        if path.exists() or path.is_symlink()
    ]
    require(not legacy_layouts, "legacy Judge or phase bundle layout")
    print(f"checked {len(entries)} native long-horizon Task/Judge adapters")


if __name__ == "__main__":
    main()
