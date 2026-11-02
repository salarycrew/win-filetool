#!/usr/bin/env python3
"""0.3.3(보안 강화): 문서 도구 묶음(.zst)을 풀었을 때의 "폴더 내용 해시"를 계산한다.

줄 `<상대 경로(/)>\t<크기>\t<SHA-256>\n`을 경로 순으로 이은 것의 SHA-256 — 앱(crates/core/src/doctools.rs `tree_sha256`)이
0.3.0~0.3.2가 풀어 둔 `lo` 폴더를 옮겨 쓰기 전에 같은 방법으로 계산해 `LO_TREE_SHA256`과 비교한다(빠진 파일·바뀐 파일·
더 들어간 파일이 있으면 쓰지 않음). 묶음은 바꾸지 않는다(읽기만).
사용: doctools-tree-hash.py <win-filetool-doctools-1.zst>
"""
import hashlib
import subprocess
import sys
import tarfile


def main(pack: str) -> int:
    p = subprocess.Popen(["zstd", "-dc", "--long=27", pack], stdout=subprocess.PIPE)
    lines = []
    with tarfile.open(fileobj=p.stdout, mode="r|") as t:
        for m in t:
            if m.isdir():
                continue
            if not m.isfile():
                print(f"일반 파일이 아닌 항목: {m.name}", file=sys.stderr)
                return 1
            name = m.name.lstrip("./")
            h = hashlib.sha256()
            f = t.extractfile(m)
            while True:
                b = f.read(1 << 20)
                if not b:
                    break
                h.update(b)
            lines.append(f"{name}\t{m.size}\t{h.hexdigest()}\n")
    if p.wait() != 0:
        return 1
    lines.sort(key=lambda s: s.split("\t", 1)[0].encode("utf-8"))
    print(hashlib.sha256("".join(lines).encode("utf-8")).hexdigest(), len(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
