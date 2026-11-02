#!/usr/bin/env python3
"""0.3.3(보안 강화): filetool-svc.exe에 빌드 확인값(SHA-256)을 써 넣는다.

서비스의 정적 값 `[b"WFT-SVC-SHA256::"][32바이트 0][b"::WFT-SVC-END:::"]`(crates/svc/src/main.rs `selfhash`)을 찾아,
그 32바이트를 0으로 둔 파일 전체의 SHA-256을 그 자리에 쓴다. 설치할 때 서비스는 복사할 파일을 같은 방법으로 계산해
실행 중인 자기 이미지의 값과 비교한다(UAC 창이 뜬 뒤 바꿔치기한 파일을 SYSTEM 서비스로 복사하지 않게).
사용: patch-svc-hash.py <filetool-svc.exe>  (이미 넣었으면 같은 값인지 확인만)
"""
import hashlib
import sys

HEAD, TAIL = b"WFT-SVC-SHA256::", b"::WFT-SVC-END:::"


def main(path: str) -> int:
    data = bytearray(open(path, "rb").read())
    hits = []
    i = data.find(HEAD)
    while i >= 0:
        if data[i + 48:i + 64] == TAIL:
            hits.append(i)
        i = data.find(HEAD, i + 1)
    if len(hits) != 1:
        print(f"확인값 자리가 {len(hits)}개입니다(1개여야 함): {path}", file=sys.stderr)
        return 1
    pos = hits[0]
    cur = bytes(data[pos + 16:pos + 48])
    data[pos + 16:pos + 48] = bytes(32)
    h = hashlib.sha256(data).digest()
    if cur != bytes(32) and cur != h:
        print(f"이미 다른 확인값이 들어 있습니다: {path}", file=sys.stderr)
        return 1
    data[pos + 16:pos + 48] = h
    open(path, "wb").write(data)
    print(f"서비스 확인값: {h.hex()} (자리 {pos:#x}) {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
