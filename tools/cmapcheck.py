"""按码位检查字体是否含字形（ttf / ttc，只用标准库）。

用途：界面出现空心方框（tofu）时，判断某个码位在字体栈里是否压根没有字形。
不要直接 glob 整个字体目录：目录里的 OFL.txt 等非字体文件会被一并喂进来，
本脚本会跳过它们，但调用方仍建议只传 *.ttf / *.ttc。

用法：
    python tools/cmapcheck.py -cp 270E,1F512,1F513 <字体1> [字体2 ...]
        逐码位判定：命中打印 YES，缺失打印 -（码位为十六进制，逗号分隔，可省 0x）。

    python tools/cmapcheck.py <字体1> [字体2 ...]
        只打印每个字体覆盖的码位数。

    python tools/cmapcheck.py -h
        显示本说明。

例：
    python tools/cmapcheck.py -cp 270E,1F512 C:/Windows/Fonts/msyh.ttc sfnt/NotoEmoji-Regular.ttf
"""

import struct
import sys

FONT_MAGICS = (b"\x00\x01\x00\x00", b"true", b"ttcf", b"OTTO")


def read_tables(data, offset=0):
    """读字体表目录；ttc 返回多个表目录的列表。"""
    tag = data[offset:offset + 4]
    if tag == b"ttcf":
        _ver, num = struct.unpack(">II", data[offset + 4:offset + 12])
        offs = struct.unpack(">%dI" % num, data[offset + 12:offset + 12 + 4 * num])
        return [read_tables(data, o) for o in offs]
    num_tables = struct.unpack(">H", data[offset + 4:offset + 6])[0]
    tables = {}
    p = offset + 12
    for _ in range(num_tables):
        t, _cs, o, ln = struct.unpack(">4sIII", data[p:p + 16])
        tables[t.decode("latin1")] = (o, ln)
        p += 16
    return tables


def cmap_codepoints(data, tables):
    """返回该表目录 cmap 里所有有字形的码位（format 4 / 12）。"""
    if "cmap" not in tables:
        return set()
    off, _ln = tables["cmap"]
    n = struct.unpack(">H", data[off + 2:off + 4])[0]
    cps = set()
    for i in range(n):
        _pid, _eid, sub = struct.unpack(">HHI", data[off + 4 + 8 * i:off + 12 + 8 * i])
        base = off + sub
        fmt = struct.unpack(">H", data[base:base + 2])[0]
        if fmt == 4:
            seg_x2 = struct.unpack(">H", data[base + 6:base + 8])[0]
            segc = seg_x2 // 2
            end = struct.unpack(">%dH" % segc, data[base + 14:base + 14 + seg_x2])
            start = struct.unpack(">%dH" % segc, data[base + 16 + seg_x2:base + 16 + 2 * seg_x2])
            delta = struct.unpack(">%dh" % segc, data[base + 16 + 2 * seg_x2:base + 16 + 3 * seg_x2])
            ro_base = base + 16 + 3 * seg_x2
            ro = struct.unpack(">%dH" % segc, data[ro_base:ro_base + seg_x2])
            for s in range(segc):
                if start[s] == 0xFFFF:
                    continue
                for c in range(start[s], min(end[s], 0xFFFE) + 1):
                    if ro[s] == 0:
                        g = (c + delta[s]) & 0xFFFF
                    else:
                        idx = ro_base + 2 * s + 2 * (c - start[s])
                        g = struct.unpack(">H", data[idx:idx + 2])[0]
                        if g:
                            g = (g + delta[s]) & 0xFFFF
                    if g:
                        cps.add(c)
        elif fmt == 12:
            ngroups = struct.unpack(">I", data[base + 12:base + 16])[0]
            p = base + 16
            for _ in range(ngroups):
                sc, ec, _sg = struct.unpack(">III", data[p:p + 12])
                p += 12
                for c in range(sc, ec + 1):
                    cps.add(c)
    return cps


def short_name(path):
    return path.replace(chr(92), "/").rsplit("/", 1)[-1]


def parse_args(argv):
    """返回 (待查码位列表, 字体路径列表)。参数非法时抛 ValueError。"""
    cps, fonts, i = [], [], 0
    while i < len(argv):
        a = argv[i]
        if a in ("-h", "--help"):
            print(__doc__.strip())
            raise SystemExit(0)
        if a == "-cp":
            i += 1
            if i >= len(argv):
                raise ValueError("-cp 后面要跟码位列表，例如 -cp 270E,1F512")
            for tok in argv[i].split(","):
                tok = tok.strip()
                if tok:
                    cps.append(int(tok, 16))
        elif a.startswith("-"):
            raise ValueError("未知参数 %s" % a)
        else:
            fonts.append(a)
        i += 1
    return cps, fonts


def main(argv):
    try:
        cps, fonts = parse_args(argv)
    except ValueError as exc:
        print("错误：%s\n" % exc)
        print(__doc__.strip())
        return 2
    if not fonts:
        print("错误：没有给字体文件\n")
        print(__doc__.strip())
        return 2
    for path in fonts:
        name = short_name(path)
        try:
            with open(path, "rb") as fh:
                data = fh.read()
        except OSError as exc:
            print("%-46s 读不了：%s" % (name, exc))
            continue
        if data[:4] not in FONT_MAGICS:
            print("%-46s 跳过：不是 ttf/ttc（魔数 %r）" % (name, data[:4]))
            continue
        tables = read_tables(data)
        if isinstance(tables, dict):
            tables = [tables]
        cpset = set()
        for t in tables:
            cpset |= cmap_codepoints(data, t)
        line = "%-46s %6d 个码位" % (name, len(cpset))
        if cps:
            line += "  " + " ".join(
                "%s:%s" % (hex(cp), "YES" if cp in cpset else "-") for cp in cps
            )
        print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
