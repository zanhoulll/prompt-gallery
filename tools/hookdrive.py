#!/usr/bin/env python3
"""hookdrive.py —— 应用内测试钩子的场景驱动（ADR-0008）。

用法：
    python tools/hookdrive.py --scene scene.txt [--data target/hook-data] [--seed 1]
    python tools/hookdrive.py < scene.txt

场景 DSL（一行一条，# 开头是注释）：
    move X Y            移动指针到客户区物理像素 (X, Y)
    down X Y            在该处按下左键
    up [X Y]            抬起左键；省略坐标则用上一次位置
    click X Y           按下并抬起（应用侧拆两帧）
    text 字符串         输入文本（整行剩余部分，保留空格）
    key R [ctrl] [shift] [alt]   按键（键名同 egui，如 R / Escape / Enter）
    shot 名称           截图到 <data>/hook/shots/<名称>.png 并等待应答
    wait 毫秒           应用侧等待（自动保存防抖请用 wait 700）
    quit                应用退出
    close               驱动侧发 WM_CLOSE（真实关窗路径，用于弹窗测试）
    kill                驱动侧强制结束进程（用于「应用应保持存活」的收尾）
    minimize            驱动侧最小化窗口（用于验证「最小化时没有新帧」的已知限制）
    expect-exit         等进程退出并以其退出码结束场景

要点：不抢前台、不用 Alt 技巧、不猜时间；每条命令等应用应答（默认 10s 超时）。
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wintypes
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_EXE = ROOT / "target" / "debug" / "prompt-gallery.exe"
WM_CLOSE = 0x0010
POLL_SECONDS = 0.01


class SceneError(Exception):
    """场景或协议错误。"""


class Ctx:
    """一次驱动运行的全部外部句柄与路径。"""

    def __init__(self, data: Path, responses: Path, err_log: Path, proc):
        self.data = data
        self.responses = responses
        self.err_log = err_log
        self.proc = proc
        self.seen = {}
        self.request_count = 0


def parse_scene(text: str):
    commands = []
    for lineno, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(None, 1)
        commands.append((lineno, parts[0], parts[1] if len(parts) > 1 else ""))
    return commands


def build_request(verb: str, rest: str, state: dict):
    """把一行场景转成请求对象；up 省略坐标时沿用上一次位置。"""
    fields = rest.split()
    if verb in ("move", "down", "click"):
        if len(fields) != 2:
            raise SceneError(verb + " 需要两个数字坐标")
        x, y = float(fields[0]), float(fields[1])
        state["pos"] = (x, y)
        return {"cmd": verb, "x": x, "y": y}
    if verb == "up":
        if len(fields) == 2:
            x, y = float(fields[0]), float(fields[1])
            state["pos"] = (x, y)
        elif state.get("pos"):
            x, y = state["pos"]
        else:
            raise SceneError("up 之前没有位置，需要显式坐标")
        return {"cmd": "up", "x": x, "y": y}
    if verb == "text":
        if not rest:
            raise SceneError("text 需要内容")
        return {"cmd": "text", "text": rest}
    if verb == "key":
        if not fields:
            raise SceneError("key 需要键名")
        request = {"cmd": "key", "key": fields[0]}
        for token in fields[1:]:
            modifier = token.lower()
            if modifier not in ("ctrl", "shift", "alt"):
                raise SceneError("未知修饰键 " + token)
            request[modifier] = True
        return request
    if verb == "shot":
        return {"cmd": "shot", "name": rest or "shot"}
    if verb == "wait":
        if len(fields) != 1:
            raise SceneError("wait 需要毫秒数")
        return {"cmd": "wait", "ms": int(float(fields[0]))}
    if verb == "quit":
        return {"cmd": "quit"}
    raise SceneError("未知命令 " + verb)


def poll_responses(ctx: Ctx):
    """重读 responses.jsonl，返回本次新出现的应答（按 seq 去重）。"""
    try:
        text = ctx.responses.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return []
    fresh = []
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        seq = obj.get("seq")
        if seq is not None and seq not in ctx.seen:
            ctx.seen[seq] = obj
            fresh.append(obj)
    return fresh


def await_response(ctx: Ctx, seq: int, timeout: float):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for obj in poll_responses(ctx):
            if obj.get("seq") == seq:
                return obj
        if ctx.proc is not None and ctx.proc.poll() is not None:
            for obj in poll_responses(ctx):
                if obj.get("seq") == seq:
                    return obj
            raise SceneError(
                "进程已退出（code=" + str(ctx.proc.returncode) + "），没有收到 seq=" + str(seq) + " 的应答"
            )
        time.sleep(POLL_SECONDS)
    raise SceneError("等待 seq=" + str(seq) + " 应答超时（" + str(timeout) + "s）")


def find_main_window(pid):
    """按进程 id 找面积最大的可见顶层窗口（测试钩子启动的应用窗口）。"""
    user32 = ctypes.windll.user32
    found = []

    def callback(hwnd, _lparam):
        if not user32.IsWindowVisible(hwnd):
            return True
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if pid is not None and owner.value != pid:
            return True
        rect = wintypes.RECT()
        user32.GetClientRect(hwnd, ctypes.byref(rect))
        area = (rect.right - rect.left) * (rect.bottom - rect.top)
        found.append((area, hwnd))
        return True

    WNDENUMPROC = ctypes.WINFUNCTYPE(ctypes.c_bool, wintypes.HWND, wintypes.LPARAM)
    user32.EnumWindows(WNDENUMPROC(callback), 0)
    if not found:
        return None
    found.sort(reverse=True)
    return found[0][1]


def terminate(proc):
    if proc is None or proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        proc.kill()


def tail(path: Path, lines: int):
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""
    return chr(10).join(text.splitlines()[-lines:])


def fail(message: str, ctx: Ctx):
    print("错误：" + message, file=sys.stderr)
    recent = [obj for obj in sorted(ctx.seen)][-5:]
    for seq in recent:
        print("  最后应答 " + json.dumps(ctx.seen[seq], ensure_ascii=False), file=sys.stderr)
    err = tail(ctx.err_log, 20)
    if err:
        print("  应用 stderr 尾部：", file=sys.stderr)
        print(err, file=sys.stderr)
    terminate(ctx.proc)
    sys.exit(1)


def launch(args, hook: Path, data: Path, out_log: Path, err_log: Path):
    env = dict(os.environ)
    env["PROMPT_GALLERY_DATA_DIR"] = str(data.resolve())
    env["PROMPT_GALLERY_TEST_HOOK"] = str(hook.resolve())
    with open(out_log, "wb") as out, open(err_log, "wb") as err:
        proc = subprocess.Popen([str(args.exe)], env=env, cwd=str(ROOT), stdout=out, stderr=err)
    return proc


def wait_hook_ready(ctx: Ctx, timeout: float):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if ctx.responses.exists():
            return
        if ctx.proc is not None and ctx.proc.poll() is not None:
            fail("应用已退出（code=" + str(ctx.proc.returncode) + "），测试钩子未启用", ctx)
        time.sleep(0.02)
    fail("应用 " + str(timeout) + "s 内没有创建 responses.jsonl（测试钩子未启用？）", ctx)


def run(args):
    data = Path(args.data)
    hook = data / "hook"
    shots = hook / "shots"
    shots.mkdir(parents=True, exist_ok=True)
    requests_path = hook / "requests.jsonl"
    responses_path = hook / "responses.jsonl"
    for path in (requests_path, responses_path):
        try:
            path.unlink()
        except FileNotFoundError:
            pass
    out_log = data / "app-out.log"
    err_log = data / "app-err.log"

    if args.seed:
        seed_exe = Path(args.seed_exe) if args.seed_exe else Path(args.exe).with_name("seed.exe")
        env = dict(os.environ)
        env["PROMPT_GALLERY_DATA_DIR"] = str(data.resolve())
        for _round in range(args.seed):
            result = subprocess.run([str(seed_exe), str(data.resolve())], env=env, cwd=str(ROOT))
            if result.returncode != 0:
                print("错误：seed 失败 code=" + str(result.returncode), file=sys.stderr)
                return 1

    proc = None if args.no_launch else launch(args, hook, data, out_log, err_log)
    ctx = Ctx(data, responses_path, err_log, proc)
    if proc is not None:
        wait_hook_ready(ctx, 20.0)

    if args.scene:
        text = Path(args.scene).read_text(encoding="utf-8")
    else:
        text = sys.stdin.read()
    commands = parse_scene(text)
    if not commands:
        fail("场景为空", ctx)

    state = {}
    started = time.monotonic()
    for lineno, verb, rest in commands:
        if verb == "close":
            hwnd = find_main_window(proc.pid if proc else None)
            if not hwnd:
                fail("第 " + str(lineno) + " 行：找不到应用窗口，无法发 WM_CLOSE", ctx)
            ctypes.windll.user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
            print("[" + str(lineno) + "] close -> WM_CLOSE")
            continue
        if verb == "minimize":
            hwnd = find_main_window(proc.pid if proc else None)
            if not hwnd:
                fail("第 " + str(lineno) + " 行：找不到应用窗口，无法最小化", ctx)
            ctypes.windll.user32.ShowWindow(hwnd, 6)
            print("[" + str(lineno) + "] minimize -> SW_MINIMIZE")
            continue
        if verb == "kill":
            terminate(proc)
            print("[" + str(lineno) + "] kill -> 已强制结束进程")
            return 0
        if verb == "expect-exit":
            code = None
            if proc is not None:
                try:
                    code = proc.wait(timeout=args.timeout)
                except subprocess.TimeoutExpired:
                    code = None
            if code is None:
                fail("第 " + str(lineno) + " 行：期望进程退出，但 " + str(args.timeout) + "s 后仍存活", ctx)
            print("[" + str(lineno) + "] expect-exit -> code=" + str(code))
            return 0
        try:
            request = build_request(verb, rest, state)
        except SceneError as exc:
            fail("第 " + str(lineno) + " 行：" + str(exc), ctx)

        ctx.request_count += 1
        seq = ctx.request_count
        line = json.dumps(request, ensure_ascii=False)
        with open(requests_path, "a", encoding="utf-8") as fh:
            fh.write(line + chr(10))
        t0 = time.monotonic()
        try:
            response = await_response(ctx, seq, args.timeout)
        except SceneError as exc:
            fail("第 " + str(lineno) + " 行 " + line + "：" + str(exc), ctx)
        elapsed_ms = (time.monotonic() - t0) * 1000.0
        ok = bool(response.get("ok"))
        detail = response.get("detail") or response.get("error") or ""
        print("[" + str(seq) + "] " + verb + (" ok " if ok else " FAIL ") + str(round(elapsed_ms)) + "ms " + detail)
        if not ok:
            fail("命令失败：" + line, ctx)
        if verb == "quit":
            code = None
            if proc is not None:
                try:
                    code = proc.wait(timeout=5.0)
                except subprocess.TimeoutExpired:
                    code = None
            if code is None:
                fail("quit 后进程未退出（可能被关窗保护拦下）；要强制结束请用驱动侧 kill", ctx)
            print("quit -> 进程退出 code=" + str(code))
            return 0

    if proc is not None and not args.keep:
        terminate(proc)
    print("场景完成：" + str(ctx.request_count) + " 条命令，总耗时 " + str(round(time.monotonic() - started, 2)) + "s")
    return 0


def main():
    parser = argparse.ArgumentParser(description="应用内测试钩子的场景驱动（ADR-0008）")
    parser.add_argument("--data", default="target/hook-data", help="数据目录（PROMPT_GALLERY_DATA_DIR）")
    parser.add_argument("--exe", default=str(DEFAULT_EXE), help="应用可执行文件（默认 debug 构建）")
    parser.add_argument("--seed", type=int, default=0, help="启动前用 seed.exe 写 N 轮示例（每轮 2 条）")
    parser.add_argument("--seed-exe", default=None, help="seed.exe 路径（默认与应用同目录）")
    parser.add_argument("--scene", default=None, help="场景文件；省略则读 stdin")
    parser.add_argument("--timeout", type=float, default=10.0, help="单条命令应答超时秒数")
    parser.add_argument("--keep", action="store_true", help="场景跑完不结束应用")
    parser.add_argument("--no-launch", action="store_true", help="不启动应用（附着到已运行实例）")
    args = parser.parse_args()
    try:
        return run(args)
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    sys.exit(main())
