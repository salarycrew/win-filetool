#!/usr/bin/env python3
"""THIRD-PARTY-NOTICES.txt 만들기(0.3.0 W1).

- Rust 의존성: `cargo metadata`(Windows x64 대상)에서 앱·CLI·서비스가 실제로 링크하는(normal) 크레이트만 모아
  이름·버전·라이선스와, 크레이트에 든 라이선스 파일 전문(같은 글은 한 번만)을 적는다.
  GPL/AGPL/LGPL만으로 된 라이선스가 있으면 멈춘다(앱에 링크하면 안 됨).
- 앱에 든 별도 프로그램: ffmpeg·LAME·dav1d·zlib(소스 묶음 안 라이선스 파일), PDFium(LICENSE·licenses/).
- 0.3.3: LibreOffice는 앱에 들어 있지 않다(사용자가 "문서 도구"를 받는 경우에만 받아짐). LibreOffice의 license.txt·NOTICE·
  LICENSE.html과 Visual C++ 런타임 안내는 문서 도구 묶음 안에 함께 있고, 여기에는 "받는 경우" 항목과 소스 받는 곳만 적는다.
- 화면(프런트엔드) 묶음에 든 npm 패키지: package.json의 dependencies와 그 하위(node_modules의 package.json·LICENSE 파일), Vite 런타임 조각.
- 시험에만 쓰는 자료(앱에 안 들어감): 한글 97 시험 파일(LibreOffice 회귀 자료, MPL-2.0) 출처.
- LGPL/MPL 부품의 "소스 받는 곳": 기본 https://github.com/salarycrew/win-filetool-sources (--source-url로 바꿈).
  자리표시({{...}})가 남으면 멈춘다. 빌드에 쓴 정확한 소스 묶음과 configure 줄은 build-windows.sh가 dist-win/sources/에 함께 둔다.

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
DEFAULT_SOURCE_URL = "https://github.com/salarycrew/win-filetool-sources"
PLACEHOLDER_RE = re.compile(r"\{\{[A-Za-z_]+\}\}")
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


def lo_version():
    """줄인 LibreOffice의 정확한 판(26.2.6.3 등) — share/registry/main.xcd의 ooSetupVersionAboutBox"""
    x = TP / "libreoffice" / "lo-trim" / "share" / "registry" / "main.xcd"
    if x.is_file():
        m = re.search(r'ooSetupVersionAboutBox"><value>([0-9.]+)<', x.read_text(encoding="utf-8", errors="replace"))
        if m:
            return m.group(1)
    return None


def pdfium_version():
    v = TP / "pdfium" / "win-x64" / "VERSION"
    if not v.is_file():
        return "chromium/7881"
    kv = dict(l.split("=", 1) for l in v.read_text().split() if "=" in l)
    return ".".join(kv.get(k, "0") for k in ("MAJOR", "MINOR", "BUILD", "PATCH"))


def npm_packages():
    """package.json dependencies와 그 하위(배포 묶음에 들어가는 것). (이름, 판, 라이선스, [라이선스 전문])"""
    nm = ROOT / "node_modules"
    top = json.loads((ROOT / "package.json").read_text(encoding="utf-8")).get("dependencies", {})
    out, seen, stack = [], set(), list(top)
    fallback = {}
    while stack:
        name = stack.pop()
        if name in seen:
            continue
        seen.add(name)
        d = nm / name
        pj = d / "package.json"
        if not pj.is_file():
            print(f"node_modules에 {name}이(가) 없습니다(npm ci 먼저)", file=sys.stderr)
            sys.exit(1)
        j = json.loads(pj.read_text(encoding="utf-8"))
        texts = [f.read_text(encoding="utf-8", errors="replace").strip() for f in sorted(d.iterdir())
                 if f.is_file() and LICENSE_RE.match(f.name) and not f.name.endswith(".spdx")]
        lic = j.get("license") or "(표기 없음)"
        if bad_license(lic):
            print(f"GPL 계열 npm 패키지: {name} {lic}", file=sys.stderr)
            sys.exit(1)
        if texts and name.startswith("@tauri-apps/"):
            fallback["@tauri-apps/"] = texts
        out.append([name, j.get("version", "?"), lic, texts])
        stack.extend((j.get("dependencies") or {}).keys())
    for o in out:
        if not o[3] and o[0].startswith("@tauri-apps/"):
            # 같은 Tauri 프로젝트의 같은 라이선스(MIT OR Apache-2.0) 전문
            o[3] = fallback.get("@tauri-apps/", [])
    return sorted((tuple(o) for o in out), key=lambda x: x[0])


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
    url = DEFAULT_SOURCE_URL
    if "--source-url" in sys.argv:
        url = sys.argv[sys.argv.index("--source-url") + 1]
    if not re.match(r"^https://\S+$", url) or PLACEHOLDER_RE.search(url):
        print(f"소스 받는 곳 주소가 올바르지 않습니다: {url}", file=sys.stderr)
        sys.exit(1)
    src = TP / "ffmpeg" / "sources"
    version = json.loads((ROOT / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))["version"]
    parts = []
    parts.append(f"파일 도구 {version} — 오픈소스 고지 (THIRD-PARTY-NOTICES)\n\n"
                 "파일 도구 자체의 코드는 MIT 라이선스입니다(Copyright (c) 2026 salarycrew). 출처를 밝히면 누구나 다시 쓸 수 있습니다.\n"
                 "소스: https://github.com/salarycrew/win-filetool\n\n"
                 "이 앱은 아래 오픈소스 부품을 씁니다. 각 부품의 저작권과 라이선스는 아래와 같습니다.\n"
                 "ffmpeg·LAME·PDFium은 앱 안에 따로 든 프로그램(별도 실행 파일·DLL)이고, 앱 본체에 링크하지 않습니다.\n"
                 "LibreOffice는 앱에 들어 있지 않습니다. 사용자가 [문서 도구]를 받는 경우에만(GitHub 릴리스에서 한 번) 이 PC에 받아지는\n"
                 "별도 프로그램이고, 그 라이선스 전문(license.txt·NOTICE·LICENSE.html)은 받은 문서 도구 폴더 안에 함께 있습니다.\n"
                 "GPL·AGPL 부품은 넣지 않았습니다(ffmpeg는 --enable-gpl 없이 빌드, LibreOffice는 poppler를 쓰는 PDF 가져오기 부품을 뺌).\n")

    # 1. 소스 받는 곳
    conf = (src / "configure.txt").read_text(encoding="utf-8") if (src / "configure.txt").is_file() else "(configure.txt 없음)"
    sums = (src / "SHA256SUMS").read_text(encoding="utf-8") if (src / "SHA256SUMS").is_file() else ""
    lo_ver = "26.2.6"
    lo_full = lo_version() or "26.2.6.3"
    parts.append(section("소스 받는 곳 (LGPL·MPL 부품)"))
    parts.append(
        f"아래 부품의 소스 코드는 다음 주소에서 받을 수 있습니다: {url}\n"
        "이 앱을 받은 날로부터 3년 동안, 위 주소에서 받지 못하면 배포자에게 연락해 같은 소스를 받을 수 있습니다.\n"
        "이 빌드에 쓴 정확한 소스 묶음과 빌드 설정은 배포 폴더의 sources\\ 에도 함께 있습니다.\n\n"
        "- ffmpeg 8.1 (LGPL-2.1-or-later): ffmpeg-8.1.tar.xz, 고치지 않은 원본. 원 출처 https://ffmpeg.org/releases/\n"
        "- LAME 3.100 (LGPL-2.0-or-later): lame-3.100.tar.gz, 고치지 않은 원본. 원 출처 https://lame.sourceforge.io/\n"
        "- dav1d 1.5.1 (BSD-2-Clause): dav1d-1.5.1.tar.xz. 원 출처 https://code.videolan.org/videolan/dav1d\n"
        "- zlib 1.3.1 (zlib): zlib-1.3.1.tar.gz. 원 출처 https://zlib.net/\n"
        f"- LibreOffice {lo_full} (MPL-2.0 등, 문서 도구를 받는 경우): 공식 Windows 설치본(x86-64 MSI)의 파일을 고치지 않고,\n"
        "  문서 → PDF 변환에 필요 없는 파일만 뺐습니다(뺀 목록: trim-libreoffice.ps1, GPL인 poppler PDF 가져오기 부품도 뺌).\n"
        f"  소스: libreoffice-{lo_full}.tar.xz, libreoffice-translations-{lo_full}.tar.xz\n"
        f"        (원 출처 https://download.documentfoundation.org/libreoffice/src/{lo_ver}/ , LibreOffice가 쓰는 외부 라이브러리 소스는 https://dev-www.libreoffice.org/src/ )\n"
        f"- PDFium {pdfium_version()} (BSD-3-Clause 등): https://pdfium.googlesource.com/pdfium/ , 바이너리 출처 https://github.com/bblanchon/pdfium-binaries (chromium/7881)\n\n"
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
    tp(f"LibreOffice {lo_full} — 문서 도구를 받는 경우 (앱에는 들어 있지 않음)",
       "문서 → PDF는 먼저 이 PC의 한글·Microsoft Office로 합니다. 그것으로 안 되는 일(Pages·Keynote·Numbers, Office가 없는 PC 등)이 있을 때\n"
       "앱이 묻고, 사용자가 [받기]를 고르면 GitHub 릴리스에서 \"문서 도구\"(줄인 LibreOffice, win-filetool-doctools-1.zst)를 한 번 받습니다.\n"
       "받은 파일은 앱에 고정한 SHA-256과 같을 때만 %LOCALAPPDATA%\\kr.salarycrew.winfiletool\\tools\\doctools\\lo\\ 에 풉니다.\n\n"
       "라이선스: Mozilla Public License 2.0 (https://www.mozilla.org/MPL/2.0/) 및 LibreOffice가 함께 싣는 서드파티 라이선스.\n"
       "전문은 문서 도구 폴더의 license.txt·NOTICE·LICENSE.html에 있습니다(LibreOffice 공식 배포본의 파일 그대로).\n"
       "Microsoft Visual C++ 런타임(msvcp140.dll·vcruntime140.dll 등)도 문서 도구 폴더 안에 있으며, Microsoft 소프트웨어 사용 조건\n"
       "(Visual Studio 재배포 파일)에 따라 LibreOffice와 함께 배포합니다.\n"
       "소스: 위 \"소스 받는 곳\"의 LibreOffice 항목(0.3.0~0.3.2와 같은 판이라 소스 저장소에 이미 있음).")
    if (TP / "pdfium" / "win-x64" / "LICENSE").is_file():
        tp("PDFium — LICENSE", (TP / "pdfium" / "win-x64" / "LICENSE").read_text(encoding="utf-8", errors="replace"))
    pd = TP / "pdfium" / "win-x64" / "licenses"
    if pd.is_dir():
        for f in sorted(pd.iterdir()):
            tp(f"PDFium — {f.name}", f.read_text(encoding="utf-8", errors="replace"))

    # 3. 화면(프런트엔드) npm 패키지
    npm = npm_packages()
    parts.append(section(f"화면(웹 UI)에 든 npm 패키지 ({len(npm)}개)"))
    for name, ver, lic, _ in npm:
        parts.append(f"- {name} {ver} — {lic}\n")
    parts.append("- Vite 런타임 조각(모듈 미리 불러오기) — MIT, Copyright (c) 2019-present, VoidZero Inc. and Vite contributors\n\n")
    seen_npm = {}
    for name, ver, lic, files in npm:
        for t in files:
            seen_npm.setdefault(hashlib.sha256(t.encode()).hexdigest(), (t, []))[1].append(f"{name} {ver}")
    for h, (t, who) in seen_npm.items():
        parts.append(f"--- 적용: {', '.join(sorted(set(who)))}\n\n{t}\n\n")

    # 4. 시험 자료(앱에 들어가지 않음)
    parts.append(section("시험에만 쓰는 자료 (앱에 들어가지 않음, 소스 저장소에만 있음)"))
    parts.append("- crates/core/tests/data/030/hwp/hangul97-3.0.hwp — LibreOffice 소스의 hwpfilter 회귀 시험 자료\n"
                 "  (https://github.com/LibreOffice/core/blob/master/hwpfilter/qa/cppunit/data/pass/hangul97-3.0.hwp),\n"
                 "  Mozilla Public License 2.0, 바꾸지 않은 원본. MPL-2.0 원문: https://www.mozilla.org/MPL/2.0/\n")

    # 5. Rust 크레이트
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
    # 빌드 자리표시가 남지 않게(0.3.0 QA-03)
    left = sorted(set(PLACEHOLDER_RE.findall(text)))
    if left:
        print(f"고지에 자리표시가 남았습니다: {', '.join(left)}", file=sys.stderr)
        sys.exit(1)
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
