#!/usr/bin/env python3
"""THIRD-PARTY-NOTICES.txt 만들기(0.3.0 W1).

- Rust 의존성: `cargo metadata`(Windows x64 대상)에서 앱·CLI·서비스가 실제로 링크하는(normal) 크레이트만 모아
  이름·버전·라이선스와, 크레이트에 든 라이선스 파일 전문(같은 글은 한 번만)을 적는다.
  GPL/AGPL/LGPL만으로 된 라이선스가 있으면 멈춘다(앱에 링크하면 안 됨).
- 앱에 든 별도 프로그램: ffmpeg·LAME·dav1d·zlib(소스 묶음 안 라이선스 파일), LibreOffice(license.txt·NOTICE), PDFium(licenses/).
- LGPL/MPL 부품의 "소스 받는 곳": 주소는 자리표시(SOURCE_URL, coordinator가 채움). 빌드에 쓴 정확한 소스 묶음과 configure 줄은
  build-windows.sh가 dist-win/sources/에 함께 둔다.

사용: python3 scripts/make-notices.py <출력 파일> [--source-url URL]
"""
import hashlib
import json
import os
import re
import subprocess
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TP = ROOT / "third_party"
SOURCE_URL_PLACEHOLDER = "{{SOURCE_URL}}"
ROOTS = {"win-filetool", "filetool-cli", "filetool-svc", "filetool-core"}
LICENSE_RE = re.compile(r"^(LICEN[CS]E|COPYING|COPYRIGHT|NOTICE|UNLICENSE)([-_.].*)?$", re.I)


def cargo_packages():
    out = subprocess.check_output(
        [os.path.expanduser("~/.cargo/bin/cargo"), "metadata", "--format-version", "1", "--locked",
         "--filter-platform", "x86_64-pc-windows-msvc"],
        cwd=ROOT,
    )
    meta = json.loads(out)
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    start = [pid for pid, p in pkgs.items() if p["name"] in ROOTS]
    seen, stack = set(), list(start)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for d in nodes[pid]["deps"]:
            if any(k["kind"] is None for k in d["dep_kinds"]):
                stack.append(d["pkg"])
    return sorted((pkgs[i] for i in seen if pkgs[i]["name"] not in ROOTS), key=lambda p: (p["name"], p["version"]))


def bad_license(expr):
    if not expr:
        return False
    e = expr.replace("/", " OR ")
    alts = [a.strip(" ()") for a in re.split(r"\bOR\b", e)]
    return all(re.search(r"\b(A?GPL|LGPL)", a) for a in alts)


def license_files(pkg):
    d = Path(pkg["manifest_path"]).parent
    files = []
    for f in sorted(d.iterdir()):
        if f.is_file() and LICENSE_RE.match(f.name):
            files.append(f)
    if pkg.get("license_file"):
        f = d / pkg["license_file"]
        if f.is_file() and f not in files:
            files.append(f)
    return files


def tar_member(tarpath, names):
    with tarfile.open(tarpath) as t:
        for m in t.getmembers():
            base = m.name.split("/", 1)[-1]
            if base in names:
                return t.extractfile(m).read().decode("utf-8", "replace")
    return f"(소스 묶음 {tarpath.name}에서 {names}를 찾지 못했습니다)"


def section(title):
    return f"\n\n{'=' * 78}\n{title}\n{'=' * 78}\n\n"


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    out = Path(sys.argv[1])
    url = SOURCE_URL_PLACEHOLDER
    if "--source-url" in sys.argv:
        url = sys.argv[sys.argv.index("--source-url") + 1]
    src = TP / "ffmpeg" / "sources"
    version = json.loads((ROOT / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))["version"]
    parts = []
    parts.append(f"파일 도구 {version} — 오픈소스 고지 (THIRD-PARTY-NOTICES)\n\n"
                 "이 앱은 아래 오픈소스 부품을 씁니다. 각 부품의 저작권과 라이선스는 아래와 같습니다.\n"
                 "ffmpeg·LAME·LibreOffice·PDFium은 앱 안에 따로 든 프로그램(별도 실행 파일·DLL)이고, 앱 본체에 링크하지 않습니다.\n"
                 "GPL·AGPL 부품은 넣지 않았습니다(ffmpeg는 --enable-gpl 없이 빌드, LibreOffice는 poppler를 쓰는 PDF 가져오기 부품을 뺌).\n")

    # 1. 소스 받는 곳
    conf = (src / "configure.txt").read_text(encoding="utf-8") if (src / "configure.txt").is_file() else "(configure.txt 없음)"
    sums = (src / "SHA256SUMS").read_text(encoding="utf-8") if (src / "SHA256SUMS").is_file() else ""
    lo_ver = "26.2.6"
    parts.append(section("소스 받는 곳 (LGPL·MPL 부품)"))
    parts.append(
        f"아래 부품의 소스 코드는 다음 주소에서 받을 수 있습니다: {url}\n"
        "이 앱을 받은 날로부터 3년 동안, 위 주소에서 받지 못하면 배포자에게 연락해 같은 소스를 받을 수 있습니다.\n"
        "이 빌드에 쓴 정확한 소스 묶음과 빌드 설정은 배포 폴더의 sources\\ 에도 함께 있습니다.\n\n"
        "- ffmpeg 8.1 (LGPL-2.1-or-later): ffmpeg-8.1.tar.xz, 고치지 않은 원본. 원 출처 https://ffmpeg.org/releases/\n"
        "- LAME 3.100 (LGPL-2.0-or-later): lame-3.100.tar.gz, 고치지 않은 원본. 원 출처 https://lame.sourceforge.io/\n"
        "- dav1d 1.5.1 (BSD-2-Clause): dav1d-1.5.1.tar.xz. 원 출처 https://code.videolan.org/videolan/dav1d\n"
        "- zlib 1.3.1 (zlib): zlib-1.3.1.tar.gz. 원 출처 https://zlib.net/\n"
        f"- LibreOffice {lo_ver} (MPL-2.0 등): 공식 Windows 설치본의 파일을 고치지 않고, 문서 → PDF 변환에 필요 없는 파일만 뺐습니다.\n"
        f"  소스: https://download.documentfoundation.org/libreoffice/src/{lo_ver}/ (뺀 목록: scripts/trim-libreoffice.ps1)\n"
        "- PDFium (BSD-3-Clause 등): https://pdfium.googlesource.com/pdfium/ , 바이너리 출처 https://github.com/bblanchon/pdfium-binaries (chromium/7881)\n\n"
        "ffmpeg 빌드 설정(configure):\n" + conf + "\n소스 묶음 SHA-256:\n" + sums
    )

    # 2. 별도 프로그램 라이선스
    def tp(title, text):
        parts.append(section(title))
        parts.append(text.strip() + "\n")

    if src.is_dir():
        tp("ffmpeg 8.1 — GNU Lesser General Public License 2.1 or later", tar_member(src / "ffmpeg-8.1.tar.xz", {"COPYING.LGPLv2.1"}))
        tp("LAME 3.100 — GNU Library General Public License 2", tar_member(src / "lame-3.100.tar.gz", {"COPYING"}))
        tp("dav1d 1.5.1 — BSD 2-Clause", tar_member(src / "dav1d-1.5.1.tar.xz", {"COPYING"}))
        tp("zlib 1.3.1 — zlib License", tar_member(src / "zlib-1.3.1.tar.gz", {"LICENSE"}))
    lo = TP / "libreoffice" / "lo-trim"
    for f in ("NOTICE", "license.txt"):
        if (lo / f).is_file():
            tp(f"LibreOffice {lo_ver} — {f}", (lo / f).read_text(encoding="utf-8", errors="replace"))
    tp("Microsoft Visual C++ 런타임 (LibreOffice와 함께 든 msvcp140.dll·vcruntime140.dll 등)",
       "Microsoft Visual C++ 재배포 가능 파일입니다. Microsoft 소프트웨어 사용 조건(Visual Studio 재배포 파일)에 따라 함께 배포합니다.")
    pd = TP / "pdfium" / "win-x64" / "licenses"
    if pd.is_dir():
        for f in sorted(pd.iterdir()):
            tp(f"PDFium — {f.name}", f.read_text(encoding="utf-8", errors="replace"))

    # 3. Rust 크레이트
    pkgs = cargo_packages()
    bad = [f"{p['name']} {p['version']} ({p['license']})" for p in pkgs if bad_license(p.get("license"))]
    if bad:
        print("GPL 계열 라이선스만 있는 크레이트가 있습니다:", *bad, sep="\n  ", file=sys.stderr)
        sys.exit(1)
    parts.append(section(f"Rust 라이브러리 ({len(pkgs)}개, 앱·CLI·검색 서비스에 링크)"))
    for p in pkgs:
        parts.append(f"- {p['name']} {p['version']} — {p.get('license') or '(라이선스 표기 없음)'}"
                     + (f" — {p['repository']}" if p.get("repository") else "") + "\n")
    texts = {}
    for p in pkgs:
        for f in license_files(p):
            t = f.read_text(encoding="utf-8", errors="replace").strip()
            h = hashlib.sha256(t.encode()).hexdigest()
            texts.setdefault(h, (t, []))[1].append(f"{p['name']} {p['version']}")
    parts.append(section(f"Rust 라이브러리 라이선스 전문 (같은 글은 한 번만, {len(texts)}개)"))
    for h, (t, who) in sorted(texts.items(), key=lambda kv: kv[1][1][0]):
        names = sorted(set(who))
        parts.append(f"--- 적용: {', '.join(names)}\n\n{t}\n\n")

    text = "".join(parts).replace("\r\n", "\n")
    # 개인 경로가 섞이지 않게
    for pat in (str(Path.home()), "C:\\Users\\"):
        if pat in text:
            print(f"고지에 로컬 경로가 들어갔습니다: {pat}", file=sys.stderr)
            sys.exit(1)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text.replace("\n", "\r\n"), encoding="utf-8-sig")
    print(f"{out}: {out.stat().st_size} B, Rust 크레이트 {len(pkgs)}개, 라이선스 전문 {len(texts)}개")


if __name__ == "__main__":
    main()
