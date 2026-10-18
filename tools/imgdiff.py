#!/usr/bin/env python
"""Prompt Gallery 截图分析：帧间差异、按色计数、扫描线色段、放大裁剪。

  python tools/imgdiff.py diff target/drag-0-idle.png target/drag-1-press.png target/drag-2-armed.png
  python tools/imgdiff.py diff target/v-0-idle.png "target/v-*.png" --vs prev
  python tools/imgdiff.py color target/v-1-click-row2.png 48 62 88 --region 0 60 300 950
  python tools/imgdiff.py scan target/n-0-idle.png --row 490 --row 79 --col 1400
  python tools/imgdiff.py crop target/v-6-fixed-click.png 0 0 320 420 --scale 2 --out target/c-v-6.png

坐标默认按 1.25 px/pt 换算（系统 125% 缩放），用 --ppp 覆盖。
"""
import argparse
import glob
import os

from PIL import Image, ImageChops


def load(path):
    return Image.open(path).convert('RGB')


def pt(x, y, ppp):
    return '%.1f,%.1f' % (x / ppp, y / ppp)


def cmd_diff(args):
    base = load(args.base)
    paths = []
    for pat in args.images:
        hits = sorted(glob.glob(pat))
        paths.extend(hits or [pat])
    prev = None
    print('base %s %s' % (args.base, base.size))
    for path in paths:
        im = load(path)
        ref = prev if (args.vs == 'prev' and prev is not None) else base
        if im.size != ref.size:
            print('%-44s SIZE %s vs %s' % (os.path.basename(path), im.size, ref.size))
            prev = im
            continue
        bbox = ImageChops.difference(ref, im).getbbox()
        if bbox is None:
            print('%-44s vs %-4s identical' % (os.path.basename(path), args.vs))
        else:
            x0, y0, x1, y1 = bbox
            print('%-44s vs %-4s bbox px=(%d,%d,%d,%d) pt=%s w=%.1f h=%.1f' % (
                os.path.basename(path), args.vs, x0, y0, x1, y1,
                pt(x0, y0, args.ppp) + '..' + pt(x1, y1, args.ppp),
                (x1 - x0) / args.ppp, (y1 - y0) / args.ppp))
        prev = im


def cmd_color(args):
    im = load(args.image)
    px = im.load()
    x0, y0, x1, y1 = args.region or (0, 0, im.size[0], im.size[1])
    r, g, b = args.rgb
    n = 0
    bx0 = by0 = None
    bx1 = by1 = None
    for y in range(y0, y1):
        for x in range(x0, x1):
            p = px[x, y]
            if abs(p[0] - r) <= args.tol and abs(p[1] - g) <= args.tol and abs(p[2] - b) <= args.tol:
                n += 1
                bx0 = x if bx0 is None else min(bx0, x)
                by0 = y if by0 is None else min(by0, y)
                bx1 = x if bx1 is None else max(bx1, x)
                by1 = y if by1 is None else max(by1, y)
    print('%s rgb(%d,%d,%d) tol=%d region=%s -> %d px' % (os.path.basename(args.image), r, g, b, args.tol, (x0, y0, x1, y1), n))
    if n:
        print('  bbox px=(%d,%d,%d,%d) pt=%s' % (bx0, by0, bx1, by1, pt(bx0, by0, args.ppp) + '..' + pt(bx1 + 1, by1 + 1, args.ppp)))


def cmd_scan(args):
    im = load(args.image)
    px = im.load()
    w, h = im.size

    def runs(fixed, horizontal):
        length = w if horizontal else h
        out = []
        cur = px[0, fixed] if horizontal else px[fixed, 0]
        start = 0
        for i in range(1, length):
            v = px[i, fixed] if horizontal else px[fixed, i]
            if v != cur:
                if i - start >= args.min_run:
                    out.append((start, i - 1, cur))
                start = i
                cur = v
        if length - start >= args.min_run:
            out.append((start, length - 1, cur))
        return out

    for y in args.row:
        print('%s row y=%d (pt %.1f)' % (os.path.basename(args.image), y, y / args.ppp))
        for a, b, c in runs(y, True):
            print('   x %4d..%4d len=%3d rgb%s  pt %.1f..%.1f' % (a, b, b - a + 1, c, a / args.ppp, (b + 1) / args.ppp))
    for x in args.col:
        print('%s col x=%d (pt %.1f)' % (os.path.basename(args.image), x, x / args.ppp))
        for a, b, c in runs(x, False):
            print('   y %4d..%4d len=%3d rgb%s  pt %.1f..%.1f' % (a, b, b - a + 1, c, a / args.ppp, (b + 1) / args.ppp))


def cmd_crop(args):
    im = load(args.image).crop((args.x0, args.y0, args.x1, args.y1))
    if args.scale != 1:
        im = im.resize((im.size[0] * args.scale, im.size[1] * args.scale), Image.NEAREST)
    out = args.out or os.path.splitext(args.image)[0] + '-crop.png'
    im.save(out)
    print('saved %s %s' % (out, im.size))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--ppp', type=float, default=1.25, help='pixels per point (默认 1.25)')
    sub = ap.add_subparsers(dest='cmd', required=True)

    d = sub.add_parser('diff', help='帧间差异 bbox')
    d.add_argument('base')
    d.add_argument('images', nargs='+', help='支持 glob')
    d.add_argument('--vs', choices=['base', 'prev'], default='base')
    d.set_defaults(func=cmd_diff)

    c = sub.add_parser('color', help='按色计数 + bbox')
    c.add_argument('image')
    c.add_argument('rgb', type=int, nargs=3, metavar=('R', 'G', 'B'))
    c.add_argument('--tol', type=int, default=6)
    c.add_argument('--region', type=int, nargs=4, metavar=('X0', 'Y0', 'X1', 'Y1'))
    c.set_defaults(func=cmd_color)

    s = sub.add_parser('scan', help='扫描线色段')
    s.add_argument('image')
    s.add_argument('--row', type=int, action='append', default=[])
    s.add_argument('--col', type=int, action='append', default=[])
    s.add_argument('--min-run', type=int, default=2)
    s.set_defaults(func=cmd_scan)

    p = sub.add_parser('crop', help='裁剪并放大')
    p.add_argument('image')
    p.add_argument('x0', type=int)
    p.add_argument('y0', type=int)
    p.add_argument('x1', type=int)
    p.add_argument('y1', type=int)
    p.add_argument('--scale', type=int, default=1)
    p.add_argument('--out')
    p.set_defaults(func=cmd_crop)

    args = ap.parse_args()
    args.func(args)


if __name__ == '__main__':
    main()
