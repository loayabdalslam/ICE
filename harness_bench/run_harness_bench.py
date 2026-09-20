#!/usr/bin/env python3
"""ICE harness-only benchmark: no LLM. Measures infrastructure."""

from __future__ import annotations

import json
import os
import resource
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "harbor_adapter"))
from ice_engine import parse_burst, run_action, default_exec, check_asserts  # noqa: E402

OUT = Path(__file__).resolve().parent / "results"
OUT.mkdir(exist_ok=True)


def ns():
    return time.perf_counter()


def timed(fn, n=1, warmup=0):
    for _ in range(warmup):
        fn()
    samples = []
    for _ in range(n):
        t0 = ns()
        fn()
        samples.append((ns() - t0) * 1000)
    return {
        "n": n,
        "ms_min": round(min(samples), 3),
        "ms_p50": round(statistics.median(samples), 3),
        "ms_mean": round(statistics.mean(samples), 3),
        "ms_max": round(max(samples), 3),
    }


def rss_mb():
    # ru_maxrss is KB on Linux
    return round(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024, 2)


def run():
    report = {
        "agent": "ICE",
        "mode": "harness-only (no LLM)",
        "host": os.uname().nodename,
        "cpu": os.cpu_count(),
        "rss_mb_start": rss_mb(),
    }
    work = Path(tempfile.mkdtemp(prefix="ice-hbench-"))
    try:
        report["benches"] = {}
        report["benches"]["startup_import_engine_ms"] = timed(
            lambda: __import__("ice_engine"), n=1
        )

        # Command latency: parse + one run echo
        burst_txt = "GOAL: ping\nBURST:\n  run echo ice\nASSERT:\n  exit 0\n"

        def parse_once():
            parse_burst(burst_txt)

        report["benches"]["burst_parse_ms"] = timed(parse_once, n=200, warmup=10)

        exec_fn = default_exec(work)

        def tool_echo():
            run_action(work, ("run", "echo ice"), exec_fn)

        report["benches"]["tool_call_echo_ms"] = timed(tool_echo, n=50, warmup=5)

        def tool_overhead():
            # parse + dispatch, command itself is tiny
            b = parse_burst(burst_txt)
            for a in b["actions"]:
                run_action(work, a, exec_fn)

        report["benches"]["tool_call_overhead_parse_exec_ms"] = timed(
            tool_overhead, n=40, warmup=5
        )

        # Process spawn
        def spawn():
            subprocess.run(["true"], check=True, capture_output=True)

        report["benches"]["process_spawn_true_ms"] = timed(spawn, n=80, warmup=5)

        # Parallel
        def parallel(k):
            t0 = ns()
            with ThreadPoolExecutor(max_workers=k) as ex:
                futs = [ex.submit(lambda: subprocess.run(["true"], capture_output=True)) for _ in range(k)]
                for f in as_completed(futs):
                    f.result()
            return (ns() - t0) * 1000

        report["benches"]["parallel_true"] = {
            str(k): round(parallel(k), 3) for k in (10, 50, 100)
        }

        # File I/O
        files = work / "tree"
        files.mkdir()
        nfiles = 2000

        def write_tree():
            d = files / "w"
            if d.exists():
                shutil.rmtree(d)
            d.mkdir()
            for i in range(nfiles):
                (d / f"f{i}.txt").write_text(f"ice-{i}\n")

        report["benches"]["file_write_2k_ms"] = timed(write_tree, n=1)
        write_tree()

        def read_tree():
            s = 0
            for p in (files / "w").iterdir():
                s += len(p.read_text())
            return s

        report["benches"]["file_read_2k_ms"] = timed(read_tree, n=3)

        # File search
        def search_py():
            hits = 0
            for p in (files / "w").rglob("*.txt"):
                if "ice-42" in p.read_text():
                    hits += 1
            return hits

        report["benches"]["file_search_python_ms"] = timed(search_py, n=3)

        def search_rg():
            subprocess.run(
                ["rg", "-l", "ice-42", str(files / "w")],
                capture_output=True,
                check=False,
            )

        report["benches"]["file_search_rg_ms"] = timed(search_rg, n=5, warmup=1)

        # Patch apply + diff
        src = work / "patchme"
        src.mkdir(exist_ok=True)
        for i in range(200):
            (src / f"m{i}.txt").write_text("aaa\nbbb\nccc\n")

        def patch_all():
            for i in range(200):
                p = src / f"m{i}.txt"
                p.write_text(p.read_text().replace("bbb", "BBB", 1))

        report["benches"]["patch_apply_200_ms"] = timed(patch_all, n=1)

        def diffs():
            subprocess.run(
                ["diff", "-ru", str(src), str(src)],
                capture_output=True,
            )

        # recreate originals vs patched copy
        orig = work / "orig"
        if orig.exists():
            shutil.rmtree(orig)
        shutil.copytree(src, orig)
        # revert orig
        for p in orig.glob("*.txt"):
            p.write_text("aaa\nbbb\nccc\n")

        def gen_diff():
            subprocess.run(["diff", "-ru", str(orig), str(src)], capture_output=True)

        report["benches"]["diff_generation_200_ms"] = timed(gen_diff, n=3)

        # Git
        gitd = work / "repo"
        gitd.mkdir()
        def git(*a):
            return subprocess.run(["git", *a], cwd=gitd, capture_output=True, text=True)

        t0 = ns()
        git("init", "-q")
        (gitd / "a.txt").write_text("hello\n")
        git("add", ".")
        git("-c", "user.email=ice@local", "-c", "user.name=ice", "commit", "-qm", "init")
        git("status", "--porcelain")
        git("diff")
        git("checkout", "-qb", "feat")
        report["benches"]["git_init_add_commit_status_branch_ms"] = round((ns() - t0) * 1000, 3)

        # Serialization
        payload = {"burst": burst_txt, "n": list(range(1000))}

        def ser():
            json.dumps(payload)
            json.loads(json.dumps(payload))

        report["benches"]["json_roundtrip_ms"] = timed(ser, n=200, warmup=20)

        # Timeout accuracy
        def timeout_case():
            t0 = ns()
            try:
                subprocess.run(["sleep", "2"], timeout=0.2)
                hit = False
            except subprocess.TimeoutExpired:
                hit = True
            return hit, (ns() - t0) * 1000

        hits = [timeout_case() for _ in range(5)]
        report["benches"]["timeout_0.2s_sleep2"] = {
            "fired": all(h for h, _ in hits),
            "ms_mean": round(statistics.mean(ms for _, ms in hits), 3),
        }

        # Cancellation
        def cancel():
            p = subprocess.Popen(["sleep", "5"])
            t0 = ns()
            p.terminate()
            p.wait(timeout=2)
            return (ns() - t0) * 1000

        report["benches"]["cancel_sleep_ms"] = timed(cancel, n=8)

        # Retry overhead
        def retry3():
            last = None
            for i in range(3):
                try:
                    if i < 2:
                        raise RuntimeError("fail")
                    last = "ok"
                except RuntimeError:
                    last = "retry"
            return last

        report["benches"]["retry_3_empty_ms"] = timed(retry3, n=200)

        # Isolation: two workspaces
        a, b = work / "iso_a", work / "iso_b"
        a.mkdir(); b.mkdir()
        (a / "x").write_text("A")
        (b / "x").write_text("B")
        report["benches"]["isolation_two_workspaces"] = {
            "a": (a / "x").read_text(),
            "b": (b / "x").read_text(),
            "isolated": (a / "x").read_text() != (b / "x").read_text(),
        }

        # Context loading: list repo src
        def load_ctx():
            files = list((ROOT / "src").rglob("*.rs"))
            text = "\n".join(p.read_text(errors="replace") for p in files)
            return len(text)

        report["benches"]["context_load_src_rs_ms"] = timed(load_ctx, n=5, warmup=1)
        report["benches"]["context_src_bytes"] = load_ctx()

        # Streaming stdout
        def stream():
            p = subprocess.Popen(
                ["python3", "-c", "print('x'*10000)"],
                stdout=subprocess.PIPE,
            )
            t0 = ns()
            first = p.stdout.read(1)
            rest = p.stdout.read()
            p.wait()
            return first, len(rest), (ns() - t0) * 1000

        s_samples = [stream()[2] for _ in range(10)]
        report["benches"]["stream_first_chunk_ms"] = {
            "ms_p50": round(statistics.median(s_samples), 3),
            "ms_mean": round(statistics.mean(s_samples), 3),
        }

        # Backpressure: huge output captured
        def huge():
            subprocess.run(
                ["python3", "-c", "print('z'*2_000_000)"],
                capture_output=True,
            )

        report["benches"]["backpressure_2mb_stdout_ms"] = timed(huge, n=3)

        # Error propagation
        def errprop():
            r = subprocess.run(["bash", "-lc", "exit 7"], capture_output=True)
            return r.returncode

        report["benches"]["error_propagation_exit7"] = {
            "code": errprop(),
            **timed(errprop, n=30),
        }

        # Cleanup
        junk = work / "junk"
        junk.mkdir()
        for i in range(500):
            (junk / f"j{i}").write_text("x")

        def cleanup():
            if junk.exists():
                shutil.rmtree(junk)
            junk.mkdir()

        report["benches"]["cleanup_500_files_ms"] = timed(cleanup, n=5)

        # Disk footprint of .ice
        ice = ROOT / ".ice"
        size = 0
        if ice.exists():
            for p in ice.rglob("*"):
                if p.is_file():
                    size += p.stat().st_size
        report["benches"]["disk_dot_ice_bytes"] = size

        # Assert path
        (work / "hello.txt").write_text("ICE_OK\n")
        burst = parse_burst(
            "GOAL: x\nBURST:\n  read hello.txt\nASSERT:\n  contains hello.txt ICE_OK\n"
        )
        st = run_action(work, burst["actions"][0], exec_fn)
        asserts = check_asserts(work, burst["asserts"], st.exit)
        report["benches"]["assert_contains"] = {"ok": all(a[1] for a in asserts), "detail": asserts}

        # Scheduler / queue: 100 jobs serial vs pool
        def queue_serial():
            for _ in range(100):
                subprocess.run(["true"], capture_output=True)

        def queue_pool():
            with ThreadPoolExecutor(8) as ex:
                list(ex.map(lambda _: subprocess.run(["true"], capture_output=True), range(100)))

        report["benches"]["queue_100_serial_ms"] = timed(queue_serial, n=3)
        report["benches"]["queue_100_pool8_ms"] = timed(queue_pool, n=3)

        report["rss_mb_end"] = rss_mb()
        report["rss_mb_delta"] = round(report["rss_mb_end"] - report["rss_mb_start"], 2)
        report["work"] = str(work)
    finally:
        shutil.rmtree(work, ignore_errors=True)

    (OUT / "harness_only.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
    return report


if __name__ == "__main__":
    run()
