#!/usr/bin/env python
"""查看 Prompt Gallery 的 SQLite 数据库：表结构、examples 内容与顺序、托管图片数量。

  python tools/dbdump.py                                   # 扫 <仓库>/target/**/*.sqlite3
  python tools/dbdump.py --dir target/uicheck-data
  python tools/dbdump.py target/uicheck-data/gallery.sqlite3

examples 有 position 列时按 position 排序输出，可直接验证拖动重排是否写库；有 locked 列时一并列出，可直接验证锁定是否写库。
"""
import argparse
import glob
import os
import sqlite3
import sys
import time

if hasattr(sys.stdout, 'reconfigure'):
    sys.stdout.reconfigure(encoding='utf-8')


def repo_root():
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def dump(path):
    print('== %s (%d bytes, mtime %s)' % (
        path, os.path.getsize(path), time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(os.path.getmtime(path)))))
    con = sqlite3.connect(path)
    try:
        tabs = [r[0] for r in con.execute("select name from sqlite_master where type='table' order by name")]
        print('   tables: %s' % (tabs or '[]'))
        for t in tabs:
            cols = [r[1] for r in con.execute('pragma table_info(%s)' % t)]
            n = con.execute('select count(*) from %s' % t).fetchone()[0]
            print('   %-14s %d rows  cols=%s' % (t, n, ','.join(cols)))
        if 'examples' in tabs:
            cols = [r[1] for r in con.execute('pragma table_info(examples)')]
            order = ' order by position' if 'position' in cols else ''
            pick = [c for c in ('id', 'title', 'position', 'locked', 'image_hash', 'image_ext') if c in cols] or ['*']
            print('   -- examples%s' % ('（按 position 排序）' if order else ''))
            for row in con.execute('select %s from examples%s' % (','.join(pick), order)):
                print('        %s' % (row,))
        imgdir = os.path.join(os.path.dirname(path), 'images')
        if os.path.isdir(imgdir):
            files = os.listdir(imgdir)
            print('   images/: %d 个托管文件' % len(files))
    finally:
        con.close()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('db', nargs='*', help='显式指定 .sqlite3 文件')
    ap.add_argument('--dir', action='append', help='扫描该目录下的 **/*.sqlite3（可多次）')
    args = ap.parse_args()

    paths = list(args.db)
    if not paths:
        for d in (args.dir or [os.path.join(repo_root(), 'target')]):
            paths += glob.glob(os.path.join(d, '**', '*.sqlite3'), recursive=True)
    paths = sorted(dict.fromkeys(paths))
    if not paths:
        print('no database found（先跑 tools/uicheck.ps1 造一个，或传 --dir）')
        return
    for p in paths:
        dump(p)


if __name__ == '__main__':
    main()
