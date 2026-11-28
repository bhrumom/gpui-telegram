#!/usr/bin/env python3
"""OpenBot 1:1 UI/UX Comprehensive Refactor — 4-Tier Opaque-Box E2E Runner & 7-Gate AST Self-Auditor.

100% Python 3.10+ standard library. Zero external packages.
Always sets sys.dont_write_bytecode = True to keep working tree free of __pycache__/.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import argparse
import ast
import datetime
import fcntl
import hashlib
import importlib
import json
import operator
import os
import py_compile
import re
import subprocess
import tempfile
import time
import traceback
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Set, Tuple

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tests.e2e_openbot_uiux.harness import (  # noqa: E402
    ALL_FEATURES,
    PROBE_SCHEMA_VERSION,
    PROBE_SUITE_NAME,
    TEST_INFRA_PREFIX_152875_SHA256,
    TEST_INFRA_PREFIX_224319_SHA256,
    TEST_REGISTRY,
    ProbeConfigError,
    UiUxE2ETestCase,
    UiUxRuntimeProbe,
    check_capability_coverage_json,
    check_cargo_toml_lean_profiles,
    check_preserved_pub_consts,
    check_workspace_plush_assets,
    configure_probe_overrides,
    run_ledger_closure_baseline,
)

LIVE_PROBE_CALL_NAMES: Set[str] = {
    "require_probe",
    "probe_section",
    "probe_scenario",
    "probe_full",
    "probe_cli_reject",
}

WORKSPACE_ORACLE_CALL_NAMES: Set[str] = {
    "check_workspace_plush_assets",
    "check_cargo_toml_lean_profiles",
    "check_capability_coverage_json",
    "check_preserved_pub_consts",
    "check_no_allow_dead_code",
    "check_immutable_governance_files",
    "run_ledger_closure_baseline",
}

ALL_BOUNDARY_CALL_NAMES: Set[str] = LIVE_PROBE_CALL_NAMES | WORKSPACE_ORACLE_CALL_NAMES

ASSERT_VERIFIER_NAMES: Set[str] = {
    "require_field",
    "require_fields",
    "assert_hex_eq",
    "assert_within",
    "assert_no_rust_panic",
    "assert_defect_eliminated",
    "assert_all_defects_eliminated",
    "assert_token_map",
}

SKIP_NAMES: Set[str] = {"skip", "xfail", "SkipTest", "skipIf", "skipUnless"}

# Dynamically constructed so SA-5 literal scan never triggers on its own blocklist
FORBIDDEN_MOCK_TOKENS: Tuple[str, ...] = (
    "PHASE_A_" + "MOCK_PASS",
    "FAKE_PROBE_" + "PASS",
    "MOCK_PASS_" + "FIXTURE",
    "DEFAULT_PASSING_" + "PROBE",
)

REQUIRED_CLI_FLAGS: Tuple[str, ...] = (
    "--self-audit",
    "--self-audit-features",
    "--tier",
    "--feature",
    "--filter",
    "--baseline",
    "--strict-dist",
    "--with-cargo",
    "--all",
    "--probe-json",
    "--probe-bin",
    "--cli-bin",
    "--json-out",
    "--json-report",
    "--verbose",
)


def _utc_now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_feature_spec(
    specs: Optional[Sequence[str]],
    flag_name: str = "--feature",
) -> List[str]:
    if specs is None:
        return []
    result: List[str] = []
    for raw in specs:
        if not raw.strip():
            raise ValueError(f"Empty {flag_name} value; expected UI-F01..UI-F20")
        for token in raw.split(","):
            tok = token.strip().upper()
            if not tok:
                raise ValueError(f"Empty token in {flag_name} {raw!r}; expected UI-F01..UI-F20")
            m_range = re.fullmatch(r"UI-F(\d{1,2})\.\.UI-F(\d{1,2})", tok)
            if m_range:
                lo, hi = int(m_range.group(1)), int(m_range.group(2))
                if lo > hi or lo < 1 or hi > 20:
                    raise ValueError(f"Invalid feature range: {tok}")
                for idx in range(lo, hi + 1):
                    fid = f"UI-F{idx:02d}"
                    if fid not in result:
                        result.append(fid)
            else:
                if tok not in ALL_FEATURES:
                    raise ValueError(f"Unknown feature ID: {tok}")
                if tok not in result:
                    result.append(tok)
    return result


def parse_tiers(raw_tiers: Optional[Sequence[str]], run_all: bool) -> List[int]:
    tiers: List[int] = []
    if raw_tiers is not None:
        for item in raw_tiers:
            if not item.strip():
                raise ValueError("Empty --tier value; expected 1, 2, 3, or 4")
            for part in item.split(","):
                p = part.strip()
                if not p:
                    raise ValueError(f"Empty token in --tier {item!r}; expected 1, 2, 3, or 4")
                val = int(p)
                if val not in (1, 2, 3, 4):
                    raise ValueError(f"Invalid tier {val}; expected 1, 2, 3, or 4")
                if val not in tiers:
                    tiers.append(val)
    if run_all:
        return [1, 2, 3, 4]
    return sorted(tiers)


def discover_and_import_tier_modules() -> List[Path]:
    pkg_dir = REPO_ROOT / "tests" / "e2e_openbot_uiux"
    tier_files = sorted(pkg_dir.glob("tier*.py"))
    for mod_file in tier_files:
        mod_name = f"tests.e2e_openbot_uiux.{mod_file.stem}"
        if mod_name not in sys.modules:
            importlib.import_module(mod_name)
    return tier_files


def _call_name(node: ast.Call) -> Optional[str]:
    if isinstance(node.func, ast.Name):
        return node.func.id
    if isinstance(node.func, ast.Attribute):
        return node.func.attr
    return None


def _decorator_name(dec: ast.expr) -> str:
    if isinstance(dec, ast.Call):
        return _decorator_name(dec.func)
    if isinstance(dec, ast.Name):
        return dec.id
    if isinstance(dec, ast.Attribute):
        return dec.attr
    return ""


_PURE_EVAL_BUILTINS: Dict[str, Any] = {
    "len": len,
    "bool": bool,
    "int": int,
    "float": float,
    "str": str,
    "abs": abs,
    "min": min,
    "max": max,
    "sum": sum,
    "all": all,
    "any": any,
    "list": list,
    "tuple": tuple,
    "set": set,
    "frozenset": frozenset,
    "dict": dict,
    "round": round,
    "ord": ord,
    "chr": chr,
}

_UNARY_OPS = {
    ast.Not: operator.not_,
    ast.UAdd: operator.pos,
    ast.USub: operator.neg,
    ast.Invert: operator.invert,
}

_BIN_OPS = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.Div: operator.truediv,
    ast.FloorDiv: operator.floordiv,
    ast.Mod: operator.mod,
    ast.BitAnd: operator.and_,
    ast.BitOr: operator.or_,
    ast.BitXor: operator.xor,
}

_CMP_OPS = {
    ast.Eq: operator.eq,
    ast.NotEq: operator.ne,
    ast.Lt: operator.lt,
    ast.LtE: operator.le,
    ast.Gt: operator.gt,
    ast.GtE: operator.ge,
    ast.Is: operator.is_,
    ast.IsNot: operator.is_not,
    ast.In: lambda a, b: a in b,
    ast.NotIn: lambda a, b: a not in b,
}


def _is_nonempty_literal_container(node: ast.expr) -> bool:
    if isinstance(node, (ast.List, ast.Tuple, ast.Set)):
        return bool(node.elts) and not any(isinstance(e, ast.Starred) for e in node.elts)
    if isinstance(node, ast.Dict):
        return bool(node.keys) and not any(k is None for k in node.keys)
    return False


def _eval_static_const(node: ast.expr) -> Tuple[bool, Any]:
    """Pure AST evaluator for statically-constant expressions (anti-cheat SA-4)."""
    if isinstance(node, ast.Constant):
        return (True, node.value)
    if isinstance(node, (ast.List, ast.Tuple, ast.Set)):
        vals: List[Any] = []
        for elt in node.elts:
            if isinstance(elt, ast.Starred):
                return (False, None)
            ok, v = _eval_static_const(elt)
            if not ok:
                return (False, None)
            vals.append(v)
        if isinstance(node, ast.List):
            return (True, vals)
        if isinstance(node, ast.Tuple):
            return (True, tuple(vals))
        try:
            return (True, set(vals))
        except TypeError:
            return (False, None)
    if isinstance(node, ast.Dict):
        out_d: Dict[Any, Any] = {}
        for k_node, v_node in zip(node.keys, node.values):
            if k_node is None:
                return (False, None)
            ok_k, kv = _eval_static_const(k_node)
            ok_v, vv = _eval_static_const(v_node)
            if not (ok_k and ok_v):
                return (False, None)
            try:
                out_d[kv] = vv
            except TypeError:
                return (False, None)
        return (True, out_d)
    if isinstance(node, ast.UnaryOp):
        op_fn = _UNARY_OPS.get(type(node.op))
        if op_fn is None:
            return (False, None)
        ok, v = _eval_static_const(node.operand)
        if not ok:
            return (False, None)
        try:
            return (True, op_fn(v))
        except Exception:
            return (False, None)
    if isinstance(node, ast.BinOp):
        ok_l, lv = _eval_static_const(node.left)
        ok_r, rv = _eval_static_const(node.right)
        if not (ok_l and ok_r):
            return (False, None)
        try:
            if isinstance(node.op, ast.Pow):
                if isinstance(rv, (int, float)) and abs(rv) <= 64:
                    return (True, operator.pow(lv, rv))
                return (False, None)
            if isinstance(node.op, ast.LShift):
                if isinstance(rv, int) and 0 <= rv <= 64:
                    return (True, operator.lshift(lv, rv))
                return (False, None)
            if isinstance(node.op, ast.RShift):
                if isinstance(rv, int) and 0 <= rv <= 64:
                    return (True, operator.rshift(lv, rv))
                return (False, None)
            op_fn = _BIN_OPS.get(type(node.op))
            if op_fn is None:
                return (False, None)
            return (True, op_fn(lv, rv))
        except Exception:
            return (False, None)
    if isinstance(node, ast.Compare):
        ok_cur, cur = _eval_static_const(node.left)
        if not ok_cur:
            return (False, None)
        for op, comp in zip(node.ops, node.comparators):
            cmp_fn = _CMP_OPS.get(type(op))
            if cmp_fn is None:
                return (False, None)
            ok_nxt, nxt = _eval_static_const(comp)
            if not ok_nxt:
                return (False, None)
            try:
                if not cmp_fn(cur, nxt):
                    return (True, False)
            except Exception:
                return (False, None)
            cur = nxt
        return (True, True)
    if isinstance(node, ast.BoolOp):
        is_or = isinstance(node.op, ast.Or)
        all_const = True
        last_val: Any = None
        for val_node in node.values:
            if is_or and _is_nonempty_literal_container(val_node):
                return (True, True)
            ok, v = _eval_static_const(val_node)
            if ok:
                last_val = v
                if is_or and bool(v):
                    return (True, v)
                if not is_or and not bool(v):
                    return (True, v)
            else:
                all_const = False
        if all_const:
            return (True, last_val)
        return (False, None)
    if isinstance(node, ast.IfExp):
        ok_t, tv = _eval_static_const(node.test)
        if ok_t:
            return _eval_static_const(node.body if tv else node.orelse)
        ok_b, bv = _eval_static_const(node.body)
        ok_e, ev = _eval_static_const(node.orelse)
        if ok_b and ok_e and bool(bv) == bool(ev):
            return (True, bool(bv))
        return (False, None)
    if (
        isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id in _PURE_EVAL_BUILTINS
        and not node.keywords
    ):
        arg_vals: List[Any] = []
        for a in node.args:
            if isinstance(a, ast.Starred):
                return (False, None)
            ok_a, av = _eval_static_const(a)
            if not ok_a:
                return (False, None)
            arg_vals.append(av)
        try:
            return (True, _PURE_EVAL_BUILTINS[node.func.id](*arg_vals))
        except Exception:
            return (False, None)
    return (False, None)


def _is_trivial_assert(node: ast.Assert) -> Optional[str]:
    t = node.test
    if isinstance(t, (ast.List, ast.Tuple, ast.Set, ast.Dict)):
        return f"literal container assert at L{node.lineno}"
    ok_const, val = _eval_static_const(t)
    if ok_const:
        return f"statically-constant assert ({val!r}) at L{node.lineno}"
    if isinstance(t, ast.Compare):
        chain = [t.left, *t.comparators]
        for left_n, right_n in zip(chain, chain[1:]):
            if ast.dump(left_n) == ast.dump(right_n):
                return f"reflexive comparison tautology at L{node.lineno}"
    return None


def _except_type_name(node: ast.expr) -> Optional[str]:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    return None


def _expr_tail_name(node: ast.expr) -> Optional[str]:
    if isinstance(node, ast.Call):
        return _expr_tail_name(node.func)
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    return None


def _is_silent_broad_except(handler: ast.ExceptHandler) -> bool:
    broad_names = ("Exception", "BaseException", "AssertionError")
    broad = False
    if handler.type is None:
        broad = True
    elif _except_type_name(handler.type) in broad_names:
        broad = True
    elif isinstance(handler.type, ast.Tuple):
        for elt in handler.type.elts:
            if _except_type_name(elt) in broad_names:
                broad = True
    if not broad:
        return False
    return not any(isinstance(sub, ast.Raise) for sub in ast.walk(handler))


def _analyze_tier_ast(
    tier_file: Path,
    tc_by_qualname: Dict[str, UiUxE2ETestCase],
) -> Tuple[List[Tuple[str, str]], int]:
    """Run AST checks for SA-3, SA-4, and SA-5 on a single tier*.py module."""
    violations: List[Tuple[str, str]] = []
    source = tier_file.read_text(encoding="utf-8")
    rel = tier_file.relative_to(REPO_ROOT).as_posix()
    try:
        tree = ast.parse(source, filename=str(tier_file))
    except SyntaxError as exc:
        return ([("SA-1", f"AST parse failed for {rel}: {exc}")], 0)
    mod_name = f"tests.e2e_openbot_uiux.{tier_file.stem}"

    for node in ast.walk(tree):
        if isinstance(node, ast.Call):
            cname = _call_name(node)
            if cname in SKIP_NAMES:
                violations.append(("SA-3", f"{rel}:L{node.lineno} forbidden skip/xfail call {cname!r}"))
        elif isinstance(node, ast.Raise) and node.exc is not None and not isinstance(node.exc, ast.Call):
            rname = _expr_tail_name(node.exc)
            if rname in SKIP_NAMES:
                violations.append(("SA-3", f"{rel}:L{node.lineno} forbidden skip raise {rname!r}"))
        elif isinstance(node, ast.AsyncFunctionDef):
            violations.append(("SA-3", f"{rel}:L{node.lineno} async def {node.name!r} unsupported"))
        elif isinstance(node, ast.ExceptHandler):
            if _is_silent_broad_except(node):
                violations.append(("SA-4", f"{rel}:L{node.lineno} broad except without re-raise"))
        elif isinstance(node, ast.ClassDef):
            for d in node.decorator_list:
                dname = _decorator_name(d)
                if dname in SKIP_NAMES:
                    violations.append(("SA-3", f"{rel}:L{node.lineno} forbidden class decorator @{dname}"))
            for cstmt in node.body:
                if isinstance(cstmt, ast.FunctionDef):
                    cdec_names = [_decorator_name(d) for d in cstmt.decorator_list]
                    for dname in cdec_names:
                        if dname in SKIP_NAMES:
                            violations.append(("SA-3", f"{rel}:L{cstmt.lineno} forbidden decorator @{dname}"))
                    if cstmt.name.startswith("test_") or "uiux_e2e_test" in cdec_names:
                        violations.append(
                            ("SA-3", f"{rel}:L{cstmt.lineno} class-scoped test {node.name}.{cstmt.name!r} unsupported")
                        )

    funcs: Dict[str, ast.FunctionDef] = {}
    decorated_tests: List[ast.FunctionDef] = []
    for stmt in tree.body:
        if isinstance(stmt, ast.FunctionDef):
            funcs[stmt.name] = stmt
            dec_names = [_decorator_name(d) for d in stmt.decorator_list]
            for dname in dec_names:
                if dname in SKIP_NAMES:
                    violations.append(("SA-3", f"{rel}:L{stmt.lineno} forbidden decorator @{dname}"))
            has_reg = "uiux_e2e_test" in dec_names
            if stmt.name.startswith("test_") and not has_reg:
                violations.append(("SA-3", f"{rel}:L{stmt.lineno} unregistered test function {stmt.name!r}"))
            if has_reg:
                decorated_tests.append(stmt)

    local_stats: Dict[str, Dict[str, Any]] = {}
    for fn_name, fn_node in funcs.items():
        non_doc = [
            s for s in fn_node.body
            if not (isinstance(s, ast.Expr) and isinstance(s.value, ast.Constant) and isinstance(s.value.value, str))
            and not isinstance(s, ast.Pass)
        ]
        direct_asserts = 0
        calls: Set[str] = set()
        for sub in ast.walk(fn_node):
            if isinstance(sub, ast.Assert):
                triv = _is_trivial_assert(sub)
                if triv:
                    violations.append(("SA-4", f"{rel}:{fn_name} {triv}"))
                else:
                    direct_asserts += 1
            elif isinstance(sub, ast.Call):
                cname = _call_name(sub)
                if cname:
                    calls.add(cname)
                    if cname in ASSERT_VERIFIER_NAMES:
                        direct_asserts += 1
        local_stats[fn_name] = {
            "non_empty": len(non_doc) > 0,
            "direct_asserts": direct_asserts,
            "calls": calls,
            "lineno": fn_node.lineno,
        }

    for t_fn in decorated_tests:
        name = t_fn.name
        st = local_stats[name]
        if not st["non_empty"]:
            violations.append(("SA-4", f"{rel}:L{st['lineno']} {name!r} has empty/pass body"))
        visited: Set[str] = {name}
        frontier: List[Tuple[str, int]] = [(name, 0)]
        total_asserts = 0
        reached_calls: Set[str] = set()
        while frontier:
            cur, depth = frontier.pop(0)
            cst = local_stats.get(cur)
            if not cst:
                continue
            total_asserts += cst["direct_asserts"]
            reached_calls |= cst["calls"]
            if depth < 3:
                for callee in cst["calls"]:
                    if callee in local_stats and callee not in visited:
                        visited.add(callee)
                        frontier.append((callee, depth + 1))
        if total_asserts < 2:
            violations.append(("SA-4", f"{rel}:{name} has only {total_asserts} real asserts/verifiers (need >= 2)"))
        if not (reached_calls & ALL_BOUNDARY_CALL_NAMES):
            violations.append(("SA-5", f"{rel}:{name} does not reach any real opaque-box boundary API"))
        tc = tc_by_qualname.get(f"{mod_name}.{name}")
        if tc is not None and any(f != "UI-F20" for f in tc.features):
            if not (reached_calls & LIVE_PROBE_CALL_NAMES):
                violations.append(("SA-5", f"{rel}:{name} ({tc.features}) missing mandatory live runtime probe call"))

    return violations, len(decorated_tests)


def run_self_audit(feature_subset: Optional[Sequence[str]] = None) -> Dict[str, Any]:
    """Execute all 7 strict AST/threshold/integrity/honesty gates SA-1..SA-7."""
    gate_errors: Dict[str, List[str]] = {f"SA-{i}": [] for i in range(1, 8)}
    pkg_dir = REPO_ROOT / "tests" / "e2e_openbot_uiux"
    runner_path = Path(__file__).resolve()
    py_files = [runner_path] + sorted(pkg_dir.glob("*.py"))

    # SA-1: py_compile into RAII temp dir + dynamic import + TEST_INFRA.md append-only prefix
    with tempfile.TemporaryDirectory(prefix="uiux_sa1_compile_") as tmp_dir:
        for idx, pf in enumerate(py_files):
            try:
                py_compile.compile(str(pf), cfile=str(Path(tmp_dir) / f"m{idx}.pyc"), doraise=True)
            except py_compile.PyCompileError as exc:
                gate_errors["SA-1"].append(f"py_compile failed for {pf.name}: {exc}")
    tier_files = sorted(pkg_dir.glob("tier*.py"))
    if not tier_files:
        gate_errors["SA-1"].append("No tier*.py modules discovered under tests/e2e_openbot_uiux/")
    for mod_file in tier_files:
        mod_name = f"tests.e2e_openbot_uiux.{mod_file.stem}"
        if mod_name not in sys.modules:
            try:
                importlib.import_module(mod_name)
            except ValueError as exc:
                gate_errors["SA-3"].append(f"Registration/ID error in {mod_file.name}: {exc}")
            except Exception as exc:
                gate_errors["SA-1"].append(
                    f"Import failed for {mod_file.name}: {type(exc).__name__}: {exc}"
                )
    infra_path = REPO_ROOT / "TEST_INFRA.md"
    if not infra_path.is_file():
        gate_errors["SA-1"].append(f"Missing required TEST_INFRA.md at {infra_path}")
    else:
        infra_bytes = infra_path.read_bytes()
        if len(infra_bytes) <= 224319:
            gate_errors["SA-1"].append(f"TEST_INFRA.md size {len(infra_bytes)} <= 224319")
        else:
            if hashlib.sha256(infra_bytes[:224319]).hexdigest() != TEST_INFRA_PREFIX_224319_SHA256:
                gate_errors["SA-1"].append("TEST_INFRA.md[:224319] SHA-256 prefix corrupted!")
            if hashlib.sha256(infra_bytes[:152875]).hexdigest() != TEST_INFRA_PREFIX_152875_SHA256:
                gate_errors["SA-1"].append("TEST_INFRA.md[:152875] SHA-256 prefix corrupted!")
            tail = infra_bytes[224319:].decode("utf-8", errors="replace")
            if not re.search(r"^## 12\.\s+OpenBot 1:1 UI/UX", tail, re.MULTILINE):
                gate_errors["SA-1"].append("TEST_INFRA.md missing appended '^## 12. OpenBot 1:1 UI/UX' heading")

    # SA-3 metadata + ID check
    seen_ids: Set[str] = set()
    tc_by_qualname: Dict[str, UiUxE2ETestCase] = {}
    for tc in TEST_REGISTRY:
        tc_by_qualname[tc.qualname] = tc
        if not re.fullmatch(r"TEST-UIUX-T[1-4]-[A-Z0-9-]+", tc.id):
            gate_errors["SA-3"].append(f"Invalid ID {tc.id!r}")
        if tc.id in seen_ids:
            gate_errors["SA-3"].append(f"Duplicate ID {tc.id!r}")
        seen_ids.add(tc.id)
        if tc.tier not in (1, 2, 3, 4) or not tc.features or any(f not in ALL_FEATURES for f in tc.features):
            gate_errors["SA-3"].append(f"Invalid metadata on {tc.id}")
        if len(tc.description.strip()) < 10:
            gate_errors["SA-3"].append(f"Short description on {tc.id}")
        if tc.tier == 3 and len(set(tc.features)) < 2:
            gate_errors["SA-2"].append(f"Tier 3 test {tc.id} tags < 2 distinct features")

    # SA-3 / SA-4 / SA-5 AST checks over tier modules + forbidden mock tokens scan
    ast_audited = 0
    for tf in tier_files:
        viols, n_fn = _analyze_tier_ast(tf, tc_by_qualname)
        ast_audited += n_fn
        for gate_id, msg in viols:
            gate_errors[gate_id].append(msg)
    for pf in py_files:
        txt = pf.read_text(encoding="utf-8")
        rel = pf.relative_to(REPO_ROOT).as_posix()
        for bad_tok in FORBIDDEN_MOCK_TOKENS:
            if bad_tok in txt:
                gate_errors["SA-5"].append(f"{rel} contains forbidden mock-pass symbol {bad_tok!r}")
        if pf.name not in ("harness.py", runner_path.name):
            try:
                pf_tree = ast.parse(txt, filename=str(pf))
            except SyntaxError:
                continue
            for node in ast.walk(pf_tree):
                if isinstance(node, ast.Constant) and isinstance(node.value, str):
                    s = node.value.strip()
                    if s == "/tmp" or s.startswith("/tmp/"):
                        gate_errors["SA-5"].append(f"{rel}:L{getattr(node, 'lineno', 0)} hardcoded /tmp literal")

    # SA-2 & SA-6 quantitative thresholds & cross-tier matrix
    target_features = list(feature_subset) if feature_subset else list(ALL_FEATURES)
    tier_totals = {1: 0, 2: 0, 3: 0, 4: 0}
    per_feat: Dict[str, Dict[str, int]] = {
        f: {"tier1": 0, "tier2": 0, "tier3": 0, "tier4": 0} for f in ALL_FEATURES
    }
    for tc in TEST_REGISTRY:
        tier_totals[tc.tier] += 1
        tk = f"tier{tc.tier}"
        for f in set(tc.features):
            per_feat[f][tk] += 1
    if not feature_subset:
        if tier_totals[1] < 100:
            gate_errors["SA-2"].append(f"Tier 1 count {tier_totals[1]} < 100")
        if tier_totals[2] < 100:
            gate_errors["SA-2"].append(f"Tier 2 count {tier_totals[2]} < 100")
        if tier_totals[3] < 20:
            gate_errors["SA-2"].append(f"Tier 3 count {tier_totals[3]} < 20")
        if tier_totals[4] < 10:
            gate_errors["SA-2"].append(f"Tier 4 count {tier_totals[4]} < 10")
        if len(TEST_REGISTRY) < 230:
            gate_errors["SA-2"].append(f"Total registered {len(TEST_REGISTRY)} < 230")
    for f in target_features:
        c = per_feat[f]
        if c["tier1"] < 5:
            gate_errors["SA-2"].append(f"{f} tier1={c['tier1']} < 5")
        if c["tier2"] < 5:
            gate_errors["SA-2"].append(f"{f} tier2={c['tier2']} < 5")
        if not feature_subset:
            if c["tier1"] < 5 or c["tier2"] < 5 or c["tier3"] < 1 or c["tier4"] < 1:
                gate_errors["SA-6"].append(f"{f} incomplete cross-tier matrix: {c}")

    # SA-7: Runner CLI contract & Phase A Honesty Gate
    runner_src = runner_path.read_text(encoding="utf-8")
    runner_tree = ast.parse(runner_src, filename=str(runner_path))
    str_literals = {
        n.value for n in ast.walk(runner_tree)
        if isinstance(n, ast.Constant) and isinstance(n.value, str)
    }
    int_returns = {
        n.value.value for n in ast.walk(runner_tree)
        if (
            isinstance(n, ast.Return)
            and isinstance(n.value, ast.Constant)
            and isinstance(n.value.value, int)
            and not isinstance(n.value.value, bool)
        )
    }
    for flag in REQUIRED_CLI_FLAGS:
        if flag not in str_literals:
            gate_errors["SA-7"].append(f"Runner CLI missing required flag {flag!r}")
    for code in (0, 1, 2):
        if code not in int_returns:
            gate_errors["SA-7"].append(f"Runner CLI missing return exit code {code}")
    phase_b_rs = REPO_ROOT / "crates" / "gpui_telegram_desktop" / "tests" / "openbot_uiux_refactor_e2e.rs"
    ready_md = REPO_ROOT / "TEST_READY.md"
    if not phase_b_rs.is_file() and ready_md.is_file():
        ready_txt = ready_md.read_text(encoding="utf-8")
        if re.search(r"OPENBOT_UIUX_REFACTOR_E2E\s*[:=]\s*READY", ready_txt, re.IGNORECASE):
            gate_errors["SA-7"].append(
                "Phase A Honesty Violation: TEST_READY.md claims OPENBOT_UIUX_REFACTOR_E2E READY before Phase B binary exists"
            )

    gates_ok = {g: (len(errs) == 0) for g, errs in gate_errors.items()}
    flat_errors = [f"[{g}] {m}" for g, errs in gate_errors.items() for m in errs]
    return {
        "passed": all(gates_ok.values()),
        "gates": gates_ok,
        "total_registered": len(TEST_REGISTRY),
        "tier_totals": {str(k): v for k, v in tier_totals.items()},
        "per_feature_counts": per_feat,
        "ast_audited_functions": ast_audited,
        "errors": flat_errors,
    }


def run_baseline(strict_dist: bool = False, with_cargo: bool = False, verbose: bool = False) -> Dict[str, Any]:
    """Execute read-only workspace baseline non-regression gate (+ optional Phase B --with-cargo)."""
    steps_spec: List[Tuple[str, Any]] = [
        ("cargo_toml_lean_profiles", check_cargo_toml_lean_profiles),
        ("openbot_plush_assets_73_webp", check_workspace_plush_assets),
        ("capability_coverage_integrity", check_capability_coverage_json),
        ("preserved_pub_const_compat", check_preserved_pub_consts),
        ("ledger_closure_gate", lambda: run_ledger_closure_baseline(strict_dist=strict_dist)),
    ]
    step_results: List[Dict[str, Any]] = []
    dist_staged = False
    all_passed = True

    for step_name, fn in steps_spec:
        t0 = time.monotonic()
        try:
            detail = fn()
            ok = True
            if step_name == "ledger_closure_gate" and isinstance(detail, dict):
                dist_staged = bool(detail.get("dist_binary_staged", False))
        except Exception as exc:
            detail = f"{type(exc).__name__}: {exc}"
            ok = False
            all_passed = False
        dt = round(time.monotonic() - t0, 4)
        step_results.append({"step": step_name, "passed": ok, "duration_sec": dt, "detail": detail})
        if verbose or not ok:
            tag = "PASS" if ok else "FAIL"
            print(f"[BASELINE {tag}] {step_name} ({dt:.3f}s)")
            if not ok:
                print(f"  -> {detail}")

    if with_cargo:
        t0 = time.monotonic()
        lock_file = Path("/tmp") / "cargo_build.lock"
        env = dict(os.environ)
        env["CARGO_BUILD_JOBS"] = "2"
        ok = False
        detail_msg = ""
        phase_b_rs = (
            REPO_ROOT / "crates" / "gpui_telegram_desktop" / "tests" / "openbot_uiux_refactor_e2e.rs"
        )
        cargo_target = "openbot_uiux_refactor_e2e" if phase_b_rs.is_file() else "openbot_ui_fusion_tests"

        def _cargo_lock_already_held(lp: Path) -> bool:
            if os.environ.get("OPENBOT_CARGO_LOCK_HELD", "").strip().lower() in ("1", "true", "yes"):
                return True
            if lp.exists():
                try:
                    target_stat = lp.stat()
                    fd_dir = Path("/proc/self/fd")
                    if fd_dir.is_dir():
                        for entry in fd_dir.iterdir():
                            try:
                                fd_stat = os.stat(entry)
                                if (fd_stat.st_dev, fd_stat.st_ino) == (
                                    target_stat.st_dev,
                                    target_stat.st_ino,
                                ):
                                    return True
                            except OSError:
                                continue
                except OSError:
                    pass
            return False

        def _run_cargo_once(run_env: Dict[str, str]) -> Tuple[bool, str]:
            try:
                proc = subprocess.run(
                    [
                        "cargo",
                        "test",
                        "--package",
                        "gpui_telegram_desktop",
                        "--test",
                        cargo_target,
                        "--",
                        "--test-threads=2",
                    ],
                    cwd=str(REPO_ROOT),
                    env=run_env,
                    capture_output=True,
                    text=True,
                    timeout=600,
                    check=False,
                )
                return (
                    proc.returncode == 0,
                    f"target={cargo_target} rc={proc.returncode}"
                    if proc.returncode == 0
                    else (proc.stderr or proc.stdout)[-500:],
                )
            except (OSError, subprocess.TimeoutExpired) as exc:
                return (False, f"target={cargo_target} {type(exc).__name__}: {exc}")
            finally:
                subprocess.run(
                    [sys.executable, "-B", str(REPO_ROOT / "scripts" / "clean_build_garbage.py")],
                    cwd=str(REPO_ROOT),
                    capture_output=True,
                    check=False,
                )

        if _cargo_lock_already_held(lock_file):
            ok, detail_msg = _run_cargo_once(env)
        else:
            with open(lock_file, "a+", encoding="utf-8") as lf:
                fcntl.flock(lf.fileno(), fcntl.LOCK_EX)
                env["OPENBOT_CARGO_LOCK_HELD"] = "1"
                try:
                    ok, detail_msg = _run_cargo_once(env)
                finally:
                    fcntl.flock(lf.fileno(), fcntl.LOCK_UN)
        if not ok:
            all_passed = False
        dt = round(time.monotonic() - t0, 4)
        step_results.append({"step": "cargo_desktop_tests", "passed": ok, "duration_sec": dt, "detail": detail_msg})

    return {
        "passed": all_passed,
        "strict_dist": strict_dist,
        "dist_binary_staged": dist_staged,
        "steps": step_results,
    }


def select_tests(
    tiers: Sequence[int],
    features: Sequence[str],
    filter_regex: Optional[re.Pattern[str]],
) -> List[UiUxE2ETestCase]:
    tier_set = set(tiers)
    feat_set = set(features)
    selected: List[UiUxE2ETestCase] = []
    for tc in sorted(TEST_REGISTRY, key=lambda item: (item.tier, item.id)):
        if tier_set and tc.tier not in tier_set:
            continue
        if feat_set and not (set(tc.features) & feat_set):
            continue
        if filter_regex is not None:
            haystack = f"{tc.id} {' '.join(tc.features)} {tc.description} {tc.qualname}"
            if "(?!" in filter_regex.pattern:
                matched = bool(filter_regex.search(tc.id)) and bool(filter_regex.search(haystack))
            else:
                matched = bool(filter_regex.search(tc.id)) or bool(filter_regex.search(haystack))
            if not matched:
                continue
        selected.append(tc)
    return selected


def run_selected_tests(selected: Sequence[UiUxE2ETestCase], verbose: bool = False) -> Dict[str, Any]:
    # Pre-validate explicit --probe-json/--probe-bin/--cli-bin so invalid explicit paths exit 2 cleanly
    with tempfile.TemporaryDirectory(prefix="openbot_uiux_probe_init_") as tmp_dir:
        probe_data, probe_src_init, probe_err = UiUxRuntimeProbe.resolve(Path(tmp_dir))

    passed = 0
    failed = 0
    results: List[Dict[str, Any]] = []
    for tc in selected:
        t0 = time.monotonic()
        err_msg: Optional[str] = None
        try:
            tc.fn()
            status = "passed"
            passed += 1
        except Exception as exc:
            status = "failed"
            failed += 1
            err_msg = f"{type(exc).__name__}: {exc}"
            if verbose:
                traceback.print_exc()
        dt = round(time.monotonic() - t0, 4)
        entry: Dict[str, Any] = {
            "id": tc.id,
            "tier": tc.tier,
            "features": list(tc.features),
            "status": status,
            "duration_sec": dt,
        }
        if err_msg is not None:
            entry["error"] = err_msg
        results.append(entry)
        if verbose:
            print(f"[{'PASS' if status == 'passed' else 'FAIL'}] {tc.id} ({dt:.3f}s)")
            if err_msg:
                print(f"  -> {err_msg}")

    probe_avail, probe_src = UiUxRuntimeProbe.probe_source_summary()
    out: Dict[str, Any] = {
        "selected": len(selected),
        "passed": passed,
        "failed": failed,
        "probe_available": probe_avail,
        "probe_source": probe_src,
        "results": results,
    }
    if probe_data is None and probe_src_init != "unavailable" and probe_err:
        out["probe_error"] = probe_err
    return out


def _write_json_report(path: Optional[str], payload: Dict[str, Any]) -> None:
    if not path:
        return
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="OpenBot 1:1 UI/UX Comprehensive Refactor — 4-Tier Opaque-Box E2E Runner & Self-Auditor"
    )
    p.add_argument("--self-audit", action="store_true", help="Run 7-gate AST, threshold & honesty self-audit")
    p.add_argument("--self-audit-features", action="append", help="Optional feature subset for --self-audit")
    p.add_argument("--tier", action="append", help="Tier(s) to run: 1,2,3,4 (repeatable or comma-separated)")
    p.add_argument("--feature", action="append", help="Feature ID(s) or range, e.g. UI-F01 or UI-F01..UI-F10")
    p.add_argument("--filter", dest="filter_expr", help="Case-insensitive regex over test ID/features/description")
    p.add_argument("--baseline", action="store_true", help="Run workspace baseline non-regression gate")
    p.add_argument("--strict-dist", action="store_true", help="Require dist/ binary staged for Gate 6.7")
    p.add_argument("--with-cargo", action="store_true", help="Phase B/M6 only: include Cargo desktop tests under flock")
    p.add_argument("--all", action="store_true", help="Run Tiers 1..4 plus --baseline")
    p.add_argument("--probe-json", help="Path to runtime UI probe JSON snapshot (or OPENBOT_UIUX_PROBE_JSON)")
    p.add_argument("--probe-bin", help="Path to compiled Phase B openbot_uiux_refactor_e2e binary")
    p.add_argument("--cli-bin", help="Path to gpui-telegram-cli binary")
    p.add_argument("--json-out", "--json-report", dest="json_out", help="Write structured JSON report to path")
    p.add_argument("-v", "--verbose", action="store_true", help="Verbose per-test output & tracebacks")
    return p


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = build_parser()
    try:
        args = parser.parse_args(argv)
    except SystemExit as exc:
        return int(exc.code) if isinstance(exc.code, int) else 2

    report: Dict[str, Any] = {
        "ok": False,
        "suite": PROBE_SUITE_NAME,
        "schema_version": PROBE_SCHEMA_VERSION,
        "timestamp": _utc_now(),
        "probe_source": "unavailable",
    }

    want_tests = bool(
        args.all
        or args.tier is not None
        or args.feature is not None
        or args.filter_expr is not None
    )
    if not (args.self_audit or args.baseline or want_tests):
        report["usage_error"] = "Specify at least one action: --self-audit, --baseline, --tier, --feature, --filter, or --all"
        print(f"ERROR: {report['usage_error']}", file=sys.stderr)
        _write_json_report(args.json_out, report)
        return 2

    try:
        tiers = parse_tiers(args.tier, args.all)
        features = parse_feature_spec(args.feature, flag_name="--feature")
        audit_features = parse_feature_spec(args.self_audit_features, flag_name="--self-audit-features")
        if args.filter_expr is not None and args.filter_expr == "":
            raise ValueError("Empty --filter regex is not allowed")
        for flag_name, flag_val in (
            ("--probe-json", args.probe_json),
            ("--probe-bin", args.probe_bin),
            ("--cli-bin", args.cli_bin),
        ):
            if flag_val is not None and not flag_val.strip():
                raise ValueError(f"Empty {flag_name} path is not allowed")
        filter_re = re.compile(args.filter_expr, re.IGNORECASE) if args.filter_expr is not None else None
    except (ValueError, re.error) as exc:
        report["usage_error"] = str(exc)
        print(f"ERROR: {exc}", file=sys.stderr)
        _write_json_report(args.json_out, report)
        return 2

    configure_probe_overrides(
        probe_json=args.probe_json,
        probe_bin=args.probe_bin,
        cli_bin=args.cli_bin,
    )

    overall_ok = True

    if args.self_audit:
        sa = run_self_audit(feature_subset=audit_features or None)
        report["self_audit"] = sa
        passed_gates = sum(1 for v in sa["gates"].values() if v)
        print(
            f"[SELF-AUDIT {'PASS' if sa['passed'] else 'FAIL'}] "
            f"{passed_gates}/7 gates | registered={sa['total_registered']} "
            f"(T1={sa['tier_totals']['1']}, T2={sa['tier_totals']['2']}, "
            f"T3={sa['tier_totals']['3']}, T4={sa['tier_totals']['4']})"
        )
        if args.verbose or not sa["passed"]:
            for g, ok_g in sa["gates"].items():
                print(f"  {g}: {'PASS' if ok_g else 'FAIL'}")
            for err in sa["errors"]:
                print(f"  ERROR: {err}")
        if not sa["passed"]:
            overall_ok = False

    if want_tests:
        try:
            discover_and_import_tier_modules()
        except Exception as exc:
            report["import_error"] = f"{type(exc).__name__}: {exc}"
            print(f"ERROR: Failed to import tier modules: {report['import_error']}", file=sys.stderr)
            _write_json_report(args.json_out, report)
            return 1
        selected = select_tests(tiers, features, filter_re)
        if not selected:
            report["usage_error"] = "0 tests matched the requested --tier/--feature/--filter selection"
            print(f"ERROR: {report['usage_error']}", file=sys.stderr)
            _write_json_report(args.json_out, report)
            return 2
        try:
            exec_res = run_selected_tests(selected, verbose=args.verbose)
        except ProbeConfigError as exc:
            report["usage_error"] = str(exc)
            print(f"ERROR: {exc}", file=sys.stderr)
            _write_json_report(args.json_out, report)
            return 2
        report["e2e_execution"] = exec_res
        report["probe_source"] = exec_res["probe_source"]
        print(
            f"[E2E {'PASS' if (exec_res['failed'] == 0 and not exec_res.get('probe_error')) else 'FAIL'}] "
            f"passed={exec_res['passed']}/{exec_res['selected']} failed={exec_res['failed']} "
            f"(probe={exec_res['probe_source']})"
        )
        if exec_res["failed"] > 0 or exec_res.get("probe_error"):
            overall_ok = False

    if args.baseline or args.all:
        base_res = run_baseline(
            strict_dist=args.strict_dist,
            with_cargo=args.with_cargo,
            verbose=args.verbose,
        )
        report["baseline"] = base_res
        print(
            f"[BASELINE {'PASS' if base_res['passed'] else 'FAIL'}] "
            f"steps={len(base_res['steps'])} dist_binary_staged={base_res['dist_binary_staged']}"
        )
        if not base_res["passed"]:
            overall_ok = False

    report["ok"] = overall_ok
    _write_json_report(args.json_out, report)
    if overall_ok:
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
