#!/usr/bin/env python3
"""앱에 넣을 도구 묶음(tools.pack) 만들기 — 형식은 crates/core/src/bundle.rs 참고.

도구마다 tar(이름 순, 시각·소유자 고정) → `zstd -19 --long=27 -T0`로 압축한 덩어리를 이어 붙이고,
목록(JSON) + 목록 길이(u64 LE) + 묶음 전체 길이(u64 LE) + "WFTTOOL1"을 붙인다.

  ffmpeg  : third_party/ffmpeg/win-x64/bin/{ffmpeg,ffprobe}.exe          (scripts/build-ffmpeg.sh)
  pdfium  : third_party/pdfium/win-x64/bin/pdfium.dll + licenses/        (scripts/fetch-pdfium.sh)
  lo      : third_party/libreoffice/lo-trim/                             (scripts/trim-libreoffice.ps1 결과)
  notices : THIRD-PARTY-NOTICES.txt                                      (scripts/make-notices.py)

사용: python3 scripts/make-tools-pack.py <출력 tools.pack> <THIRD-PARTY-NOTICES.txt>
"""
import hashlib
import io
import json
import subprocess
import sys
import tarfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TP = ROOT / "third_party"
MAGIC = b"WFTTOOL1"
FIXED_MTIME = 1767225600  # 2026-01-01T00:00:00Z


def entries(notices):
    ff = TP / "ffmpeg" / "win-x64" / "bin"
    pd = TP / "pdfium" / "win-x64"
    lo = TP / "libreoffice" / "lo-trim"
    yield "ffmpeg", [(ff / "ffmpeg.exe", "ffmpeg.exe"), (ff / "ffprobe.exe", "ffprobe.exe")]
    files = [(pd / "bin" / "pdfium.dll", "pdfium.dll")]
    files += [(f, f"licenses/{f.name}") for f in sorted((pd / "licenses").iterdir())]
    yield "pdfium", files
    yield "lo", [(f, f.relative_to(lo).as_posix()) for f in sorted(lo.rglob("*")) if f.is_file()]
    yield "notices", [(Path(notices), "THIRD-PARTY-NOTICES.txt")]


def tar_bytes(files):
    buf = io.BytesIO()
    total = 0
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.GNU_FORMAT) as t:
        for src, name in files:
            if not src.is_file():
                raise SystemExit(f"없음: {src}")
            ti = tarfile.TarInfo(name)
            ti.size = src.stat().st_size
            ti.mtime = FIXED_MTIME
            ti.mode = 0o644
            ti.uid = ti.gid = 0
            ti.uname = ti.gname = ""
            with open(src, "rb") as fh:
                t.addfile(ti, fh)
            total += ti.size
    return buf.getvalue(), total


def zstd(data):
    p = subprocess.run(["zstd", "-19", "--long=27", "-T0", "-q", "-c"], input=data, stdout=subprocess.PIPE, check=True)
    return p.stdout


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    out, notices = Path(sys.argv[1]), sys.argv[2]
    blobs, index, h = [], [], hashlib.sha256()
    offset = 0
    for name, files in entries(notices):
        t0 = time.time()
        raw, total = tar_bytes(files)
        z = zstd(raw)
        h.update(name.encode() + b"\0" + z)
        index.append({"name": name, "offset": offset, "len": len(z), "files": len(files), "bytes": total})
        blobs.append(z)
        offset += len(z)
        print(f"{name}: 파일 {len(files)}개, {total} B → {len(z)} B ({time.time() - t0:.1f}초)")
    idx = json.dumps({"version": 1, "hash": h.hexdigest()[:16], "tools": index}, separators=(",", ":")).encode()
    pack_len = offset + len(idx) + 24
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "wb") as f:
        for b in blobs:
            f.write(b)
        f.write(idx)
        f.write(len(idx).to_bytes(8, "little"))
        f.write(pack_len.to_bytes(8, "little"))
        f.write(MAGIC)
    print(f"{out}: {out.stat().st_size} B, hash {h.hexdigest()[:16]}")


if __name__ == "__main__":
    main()
