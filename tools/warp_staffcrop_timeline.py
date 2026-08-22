#!/usr/bin/env python3
"""Rewrite video/fade times in a .staffcrop zip: t' = a*t + b.

Does not touch audio clips (they are the reference clock). Default writes a
sibling *.warped.staffcrop and leaves the original alone.

No arguments (or --gui / a packed .exe) opens a small window.
CLI examples:

  python warp_staffcrop_timeline.py piece.staffcrop --old 2700 --new 2694.5
  python warp_staffcrop_timeline.py piece.staffcrop --pair 0,0 --pair 2700,2694.5
  python warp_staffcrop_timeline.py piece.staffcrop --a 0.998 --b 0
"""

from __future__ import annotations

import argparse
import io
import json
import sys
import zipfile
from pathlib import Path


class WarpError(Exception):
    pass


def map_t(t: float, a: float, b: float) -> float:
    return a * float(t) + b


def solve_ab(pairs: list[tuple[float, float]]) -> tuple[float, float]:
    if len(pairs) == 1:
        old, new = pairs[0]
        if abs(old) < 1e-12:
            raise WarpError("只有一个对照点且旧时刻为 0 时无法求出 a, 请再给一个点或直接填 a")
        return new / old, 0.0
    (o1, n1), (o2, n2) = pairs[0], pairs[1]
    den = o2 - o1
    if abs(den) < 1e-12:
        raise WarpError("两个对照点的旧时刻必须不同")
    a = (n2 - n1) / den
    b = n1 - a * o1
    return a, b


def warp_video(video: dict, a: float, b: float) -> dict:
    clips = video.get("video_clips") or []
    fades = video.get("fades") or []
    for c in clips:
        c["start"] = map_t(c["start"], a, b)
        c["end"] = map_t(c["end"], a, b)
    for f in fades:
        f["start"] = map_t(f["start"], a, b)
        f["end"] = map_t(f["end"], a, b)
    if "playhead" in video:
        video["playhead"] = max(0.0, map_t(video["playhead"], a, b))
    for c in clips:
        if c["end"] < c["start"]:
            c["start"], c["end"] = c["end"], c["start"]
        if c["start"] < 0.0:
            c["start"] = 0.0
        if c["end"] < c["start"]:
            c["end"] = c["start"]
    for f in fades:
        if f["end"] < f["start"]:
            f["start"], f["end"] = f["end"], f["start"]
        f["start"] = max(0.0, f["start"])
        f["end"] = max(f["start"], f["end"])
    video["video_clips"] = clips
    video["fades"] = fades
    return video


def summarize(video: dict) -> str:
    clips = video.get("video_clips") or []
    audio = video.get("audio_clips") or []
    audio_end = sum(float(x.get("duration") or 0.0) for x in audio)
    if not clips:
        return f"video clips: 0, audio_end={audio_end:.6f}"
    return (
        f"clips={len(clips)} first={clips[0]['start']:.6f}..{clips[0]['end']:.6f} "
        f"last_end={clips[-1]['end']:.6f} audio_end={audio_end:.6f}"
    )


def parse_pair(s: str) -> tuple[float, float]:
    parts = s.replace(":", ",").split(",")
    if len(parts) != 2:
        raise argparse.ArgumentTypeError("expected OLD,NEW")
    return float(parts[0].strip()), float(parts[1].strip())


def parse_time(s: str) -> float:
    text = s.strip().replace("：", ":")
    if not text:
        raise WarpError("时刻不能为空")
    if ":" in text:
        parts = text.split(":")
        try:
            nums = [float(p) for p in parts]
        except ValueError as e:
            raise WarpError(f"无法解析时刻: {s}") from e
        if len(nums) == 2:
            return nums[0] * 60.0 + nums[1]
        if len(nums) == 3:
            return nums[0] * 3600.0 + nums[1] * 60.0 + nums[2]
        raise WarpError("时刻格式用秒, 或 mm:ss / hh:mm:ss")
    try:
        return float(text)
    except ValueError as e:
        raise WarpError(f"无法解析时刻: {s}") from e


def warp_project(src: Path, a: float, b: float, out: Path | None, dry: bool) -> str:
    if a <= 0:
        raise WarpError(f"a 必须 > 0 (当前 {a})")
    if not src.is_file():
        raise WarpError(f"不是文件: {src}")

    lines: list[str] = []
    with zipfile.ZipFile(src, "r") as zin:
        try:
            raw = zin.read("project.json")
        except KeyError as e:
            raise WarpError("zip 内没有 project.json") from e
        meta = json.loads(raw.decode("utf-8"))
        video = meta.get("video") or {}
        before = summarize(video)
        meta["video"] = warp_video(video, a, b)
        after = summarize(meta["video"])
        new_json = json.dumps(meta, ensure_ascii=False, indent=2) + "\n"
        lines.append(f"t' = {a:.12g}*t + {b:.12g}")
        lines.append(f"before: {before}")
        lines.append(f"after:  {after}")
        if dry:
            lines.append("(dry run, 未写文件)")
            return "\n".join(lines)

        if out is None:
            out = src.with_name(src.stem + ".warped.staffcrop")
        out = out.resolve()
        if out == src.resolve():
            raise WarpError("拒绝覆盖原工程, 请另选输出路径")

        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w") as zout:
            for info in zin.infolist():
                data = new_json.encode("utf-8") if info.filename == "project.json" else zin.read(info.filename)
                dest = zipfile.ZipInfo(filename=info.filename, date_time=info.date_time)
                dest.compress_type = info.compress_type
                dest.external_attr = info.external_attr
                zout.writestr(dest, data)

    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(buf.getvalue())
    lines.append(f"wrote {out}")
    return "\n".join(lines)


def cli_main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description="Apply t' = a*t + b to video/fade times in a .staffcrop")
    p.add_argument("input", type=Path, help=".staffcrop zip")
    p.add_argument("-o", "--out", type=Path, help="output path (default: <stem>.warped.staffcrop)")
    p.add_argument("--a", type=float, help="scale")
    p.add_argument("--b", type=float, default=None, help="offset seconds (default 0 if --a or --old)")
    p.add_argument("--old", type=float, help="one old-clock time; with --new and b=0")
    p.add_argument("--new", type=float, help="where --old should land")
    p.add_argument(
        "--pair",
        action="append",
        type=parse_pair,
        default=[],
        metavar="OLD,NEW",
        help="correspondence; pass twice to solve a and b",
    )
    p.add_argument("--dry", action="store_true", help="print mapping, do not write")
    p.add_argument("--gui", action="store_true", help="open the window instead of CLI")
    args = p.parse_args(argv)

    if args.gui:
        return run_gui()

    pairs: list[tuple[float, float]] = list(args.pair)
    if args.old is not None or args.new is not None:
        if args.old is None or args.new is None:
            print("--old and --new must be used together", file=sys.stderr)
            return 1
        pairs.append((args.old, args.new))

    try:
        if args.a is not None:
            a = args.a
            b = 0.0 if args.b is None else args.b
            if pairs:
                raise WarpError("do not mix --a with --old/--pair")
        elif pairs:
            if args.b is not None and len(pairs) == 1:
                a = pairs[0][1] / pairs[0][0] if abs(pairs[0][0]) > 1e-12 else 1.0
                b = args.b
            else:
                a, b = solve_ab(pairs)
        else:
            raise WarpError("need --a, or --old/--new, or --pair")
        text = warp_project(args.input, a, b, args.out, args.dry)
    except WarpError as e:
        print(str(e), file=sys.stderr)
        return 1
    print(text)
    return 0


def ui_font() -> tuple[str, int]:
    if sys.platform == "darwin":
        return ("PingFang SC", 13)
    return ("Microsoft YaHei UI", 10)


def run_gui() -> int:
    import tkinter as tk
    from tkinter import filedialog, messagebox, ttk

    font = ui_font()
    root = tk.Tk()
    root.title("工程时间轴对齐")
    root.minsize(560, 520)
    root.geometry("640x580")
    try:
        root.tk.call("tk", "scaling", 1.15)
    except tk.TclError:
        pass

    style = ttk.Style()
    if sys.platform == "win32" and "vista" in style.theme_names():
        style.theme_use("vista")
    style.configure(".", font=font)
    style.configure("TLabelframe.Label", font=font)
    style.configure("Hint.TLabel", foreground="#475569")
    style.configure("Run.TButton", font=(font[0], font[1] + 1, "bold"), padding=(12, 6))

    src_var = tk.StringVar()
    out_var = tk.StringVar()
    dry_var = tk.BooleanVar(value=False)
    mode_var = tk.StringVar(value="one")
    old_var = tk.StringVar()
    new_var = tk.StringVar()
    p1_old = tk.StringVar(value="0")
    p1_new = tk.StringVar(value="0")
    p2_old = tk.StringVar()
    p2_new = tk.StringVar()
    a_var = tk.StringVar(value="1")
    b_var = tk.StringVar(value="0")

    def default_out_from_src() -> None:
        p = Path(src_var.get().strip())
        if p.suffix.lower() == ".staffcrop":
            out_var.set(str(p.with_name(p.stem + ".warped.staffcrop")))

    def browse_src() -> None:
        path = filedialog.askopenfilename(
            title="选择 .staffcrop 工程",
            filetypes=[("Score Sync 工程", "*.staffcrop"), ("全部", "*.*")],
        )
        if path:
            src_var.set(path)
            default_out_from_src()

    def browse_out() -> None:
        path = filedialog.asksaveasfilename(
            title="另存对齐后的工程",
            defaultextension=".staffcrop",
            filetypes=[("Score Sync 工程", "*.staffcrop"), ("全部", "*.*")],
        )
        if path:
            out_var.set(path)

    def set_mode_state(*_args: object) -> None:
        mode = mode_var.get()
        pairs = (
            (one_entries, mode == "one"),
            (two_entries, mode == "two"),
            (ab_entries, mode == "ab"),
        )
        for widgets, on in pairs:
            st = "normal" if on else "disabled"
            for w in widgets:
                w.configure(state=st)

    def resolve_ab() -> tuple[float, float]:
        mode = mode_var.get()
        if mode == "one":
            return solve_ab([(parse_time(old_var.get()), parse_time(new_var.get()))])
        if mode == "two":
            return solve_ab(
                [
                    (parse_time(p1_old.get()), parse_time(p1_new.get())),
                    (parse_time(p2_old.get()), parse_time(p2_new.get())),
                ]
            )
        try:
            a = float(a_var.get().strip())
            b = float(b_var.get().strip())
        except ValueError as e:
            raise WarpError("a, b 必须是数字") from e
        return a, b

    def append_log(text: str) -> None:
        log.configure(state="normal")
        log.insert("end", text.rstrip() + "\n")
        log.see("end")
        log.configure(state="disabled")

    def run_warp() -> None:
        src = Path(src_var.get().strip())
        out_s = out_var.get().strip()
        out = Path(out_s) if out_s else None
        dry = bool(dry_var.get())
        try:
            a, b = resolve_ab()
            text = warp_project(src, a, b, out, dry)
        except WarpError as e:
            append_log(f"失败: {e}")
            messagebox.showerror("无法对齐", str(e), parent=root)
            return
        append_log(text)
        if dry:
            messagebox.showinfo("预览", text, parent=root)
        else:
            messagebox.showinfo("已写出", text, parent=root)

    pad = {"padx": 10, "pady": 6}
    frm = ttk.Frame(root, padding=12)
    frm.pack(fill="both", expand=True)

    ttk.Label(frm, text="把旧工程的谱面时间轴按 t' = a·t + b 写进新文件.").grid(
        row=0, column=0, columnspan=3, sticky="w"
    )
    ttk.Label(
        frm,
        text="只改视频轨 / 淡入淡出 / 播放头, 不动音频. 默认不覆盖原工程.",
        style="Hint.TLabel",
    ).grid(row=1, column=0, columnspan=3, sticky="w", pady=(0, 8))

    ttk.Label(frm, text="工程").grid(row=2, column=0, sticky="e")
    ttk.Entry(frm, textvariable=src_var).grid(row=2, column=1, sticky="ew", **pad)
    ttk.Button(frm, text="浏览…", command=browse_src).grid(row=2, column=2, sticky="ew")

    ttk.Label(frm, text="输出").grid(row=3, column=0, sticky="e")
    ttk.Entry(frm, textvariable=out_var).grid(row=3, column=1, sticky="ew", **pad)
    ttk.Button(frm, text="浏览…", command=browse_out).grid(row=3, column=2, sticky="ew")

    ttk.Checkbutton(frm, text="只预览, 不写文件", variable=dry_var).grid(
        row=4, column=1, sticky="w", pady=(0, 4)
    )

    box = ttk.LabelFrame(frm, text="标定", padding=10)
    box.grid(row=5, column=0, columnspan=3, sticky="nsew", pady=8)
    box.columnconfigure(1, weight=1)
    box.columnconfigure(3, weight=1)

    ttk.Radiobutton(
        box,
        text="一个对照点 (开头不动, 越到后面偏得越多时用这个)",
        variable=mode_var,
        value="one",
        command=set_mode_state,
    ).grid(row=0, column=0, columnspan=4, sticky="w")

    l1 = ttk.Label(box, text="工程里的时刻")
    e1 = ttk.Entry(box, textvariable=old_var, width=14)
    l2 = ttk.Label(box, text="应对齐到")
    e2 = ttk.Entry(box, textvariable=new_var, width=14)
    l1.grid(row=1, column=0, sticky="e", padx=(24, 6), pady=2)
    e1.grid(row=1, column=1, sticky="w", pady=2)
    l2.grid(row=1, column=2, sticky="e", padx=(12, 6), pady=2)
    e2.grid(row=1, column=3, sticky="w", pady=2)
    one_entries = [e1, e2]
    hint1 = ttk.Label(
        box,
        text="秒, 或 mm:ss / hh:mm:ss. 乐谱落后于音乐时, 右侧应小于左侧.",
        style="Hint.TLabel",
    )
    hint1.grid(row=2, column=0, columnspan=4, sticky="w", padx=(24, 0), pady=(0, 8))

    ttk.Radiobutton(
        box,
        text="两个对照点 (同时解 a 和 b)",
        variable=mode_var,
        value="two",
        command=set_mode_state,
    ).grid(row=3, column=0, columnspan=4, sticky="w")

    l3 = ttk.Label(box, text="点 1 旧")
    e3 = ttk.Entry(box, textvariable=p1_old, width=10)
    l4 = ttk.Label(box, text="新")
    e4 = ttk.Entry(box, textvariable=p1_new, width=10)
    l5 = ttk.Label(box, text="点 2 旧")
    e5 = ttk.Entry(box, textvariable=p2_old, width=10)
    l6 = ttk.Label(box, text="新")
    e6 = ttk.Entry(box, textvariable=p2_new, width=10)
    l3.grid(row=4, column=0, sticky="e", padx=(24, 6), pady=2)
    e3.grid(row=4, column=1, sticky="w")
    l4.grid(row=4, column=2, sticky="e", padx=(12, 6))
    e4.grid(row=4, column=3, sticky="w")
    l5.grid(row=5, column=0, sticky="e", padx=(24, 6), pady=2)
    e5.grid(row=5, column=1, sticky="w")
    l6.grid(row=5, column=2, sticky="e", padx=(12, 6))
    e6.grid(row=5, column=3, sticky="w")
    two_entries = [e3, e4, e5, e6]

    ttk.Radiobutton(
        box,
        text="直接填 a, b",
        variable=mode_var,
        value="ab",
        command=set_mode_state,
    ).grid(row=6, column=0, columnspan=4, sticky="w", pady=(8, 0))

    l7 = ttk.Label(box, text="a")
    e7 = ttk.Entry(box, textvariable=a_var, width=14)
    l8 = ttk.Label(box, text="b")
    e8 = ttk.Entry(box, textvariable=b_var, width=14)
    l7.grid(row=7, column=0, sticky="e", padx=(24, 6), pady=2)
    e7.grid(row=7, column=1, sticky="w")
    l8.grid(row=7, column=2, sticky="e", padx=(12, 6))
    e8.grid(row=7, column=3, sticky="w")
    ab_entries = [e7, e8]

    ttk.Button(frm, text="开始对齐", style="Run.TButton", command=run_warp).grid(
        row=6, column=0, columnspan=3, pady=8
    )

    ttk.Label(frm, text="结果").grid(row=7, column=0, sticky="nw")
    log_fr = ttk.Frame(frm)
    log_fr.grid(row=7, column=1, columnspan=2, sticky="nsew", pady=4)
    log_fr.columnconfigure(0, weight=1)
    log_fr.rowconfigure(0, weight=1)
    log = tk.Text(
        log_fr, height=8, wrap="word", font=font, state="disabled", relief="solid", borderwidth=1
    )
    log.grid(row=0, column=0, sticky="nsew")
    scroll = ttk.Scrollbar(log_fr, command=log.yview)
    log.configure(yscrollcommand=scroll.set)
    scroll.grid(row=0, column=1, sticky="ns")

    frm.columnconfigure(1, weight=1)
    frm.rowconfigure(7, weight=1)

    set_mode_state()
    root.after(50, lambda: e1.focus_set())

    if len(sys.argv) >= 2:
        maybe = Path(sys.argv[1])
        if maybe.suffix.lower() == ".staffcrop" and maybe.is_file():
            src_var.set(str(maybe.resolve()))
            default_out_from_src()

    root.mainloop()
    return 0


def main() -> int:
    argv = sys.argv[1:]
    force_gui = "--gui" in argv
    cli_argv = [a for a in argv if a != "--gui"]
    if force_gui or not cli_argv:
        return run_gui()
    return cli_main(cli_argv)


if __name__ == "__main__":
    raise SystemExit(main())
