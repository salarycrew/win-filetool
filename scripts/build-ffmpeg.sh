#!/usr/bin/env bash
# 앱에 넣는 ffmpeg/ffprobe(Windows x64)를 Linux 빌드 PC에서 llvm-mingw로 교차 빌드한다. LGPL 최소 빌드.
#
# - 라이선스: LGPL-2.1-or-later. --enable-gpl / --enable-nonfree / --enable-version3 는 쓰지 않는다.
#   H.264 인코딩은 Windows Media Foundation(h264_mf)에 맡긴다(libx264 같은 GPL 부품 없음).
# - 함께 넣는 라이브러리: LAME 3.100(LGPL-2.0, MP3 인코딩), dav1d 1.5.1(BSD-2, AV1 디코딩), zlib 1.3.1(zlib).
# - 0.2 `media.rs`/`imageconv.rs`(HEIC: hevc 디먹서 + xstack/overlay), 0.3 W4(앞뒤 자르기 -ss/-to, 소리 빼기 -an,
#   돌리기 transpose/hflip/vflip, 화면 한 장 JPG(mjpeg), 크기 바꾸기 scale)를 같은 빌드로 쓴다.
# - 개인 경로가 exe에 들어가지 않도록: configure는 작업 폴더 안에서 상대 경로로 부르고, 라이브러리 경로도 상대 경로로 준다
#   (`ffmpeg -version`의 configuration 줄에 그대로 보이므로).
#
# 결과:
#   third_party/ffmpeg/win-x64/bin/ffmpeg.exe, ffprobe.exe
#   third_party/ffmpeg/sources/  — 빌드에 쓴 소스 묶음 그대로(ffmpeg·LAME·dav1d·zlib) + configure.txt(configure 줄)
#                                  (LGPL 소스 제공용. build-windows.sh가 dist-win/sources/로 복사한다)
# 작업 폴더: FFMPEG_WORK(기본 target/ffmpeg-build). 필요: curl, make, python3 + meson·ninja(pip install --user meson ninja)
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
W="${FFMPEG_WORK:-$ROOT/target/ffmpeg-build}"
OUT="$ROOT/third_party/ffmpeg"
mkdir -p "$W"/{dl,src,tmp} "$OUT/win-x64/bin" "$OUT/sources"
export TMPDIR="$W/tmp"
export PATH="$HOME/.local/bin:$PATH"

FFMPEG_VER=8.1
LAME_VER=3.100
DAV1D_VER=1.5.1
ZLIB_VER=1.3.1
NASM_VER=2.16.03
LLVM_MINGW=20261020

dl() { [ -s "$W/dl/$2" ] || curl -sSfL -o "$W/dl/$2" "$1"; }
dl "https://github.com/mstorsjo/llvm-mingw/releases/download/$LLVM_MINGW/llvm-mingw-$LLVM_MINGW-ucrt-ubuntu-22.04-x86_64.tar.xz" llvm-mingw.tar.xz
dl "https://www.nasm.us/pub/nasm/releasebuilds/$NASM_VER/nasm-$NASM_VER.tar.xz" "nasm-$NASM_VER.tar.xz"
dl "https://downloads.sourceforge.net/project/lame/lame/$LAME_VER/lame-$LAME_VER.tar.gz" "lame-$LAME_VER.tar.gz"
dl "https://downloads.videolan.org/pub/videolan/dav1d/$DAV1D_VER/dav1d-$DAV1D_VER.tar.xz" "dav1d-$DAV1D_VER.tar.xz"
dl "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VER.tar.xz" "ffmpeg-$FFMPEG_VER.tar.xz"
dl "https://zlib.net/fossils/zlib-$ZLIB_VER.tar.gz" "zlib-$ZLIB_VER.tar.gz"

for t in meson ninja make; do command -v $t >/dev/null || { echo "필요한 도구가 없습니다: $t" >&2; exit 1; }; done

cd "$W/src"
[ -d llvm-mingw ] || { mkdir llvm-mingw && tar xf ../dl/llvm-mingw.tar.xz -C llvm-mingw --strip-components=1; }
[ -x "$W/host/bin/nasm" ] || { rm -rf nasm; mkdir nasm && tar xf "../dl/nasm-$NASM_VER.tar.xz" -C nasm --strip-components=1
  (cd nasm && ./configure --prefix="$W/host" >/dev/null && make -j"$(nproc)" >/dev/null && make install >/dev/null); }
export PATH="$W/src/llvm-mingw/bin:$W/host/bin:$PATH"
H=x86_64-w64-mingw32
P="$W/prefix"
# 소스 경로가 디버그 문자열(__FILE__)로 들어가지 않게
MAP="-ffile-prefix-map=$W=."

[ -f "$P/lib/libmp3lame.a" ] || { rm -rf lame; mkdir lame && tar xf "../dl/lame-$LAME_VER.tar.gz" -C lame --strip-components=1
  (cd lame && ./configure --host=$H --prefix="$P" --disable-shared --enable-static --disable-frontend --disable-decoder --enable-nasm \
     CC=$H-clang CFLAGS="-O2 $MAP" >/dev/null && make -j"$(nproc)" >/dev/null && make install >/dev/null); }

[ -f "$P/lib/libz.a" ] || { rm -rf zlib; mkdir zlib && tar xf "../dl/zlib-$ZLIB_VER.tar.gz" -C zlib --strip-components=1
  (cd zlib && make -f win32/Makefile.gcc PREFIX=$H- CC=$H-clang AR=$H-ar RC=$H-windres CFLAGS="-O2 $MAP" -j"$(nproc)" >/dev/null \
     && make -f win32/Makefile.gcc install BINARY_PATH="$P/bin" INCLUDE_PATH="$P/include" LIBRARY_PATH="$P/lib" >/dev/null); }

if [ ! -f "$P/lib/libdav1d.a" ]; then
  rm -rf dav1d; mkdir dav1d && tar xf "../dl/dav1d-$DAV1D_VER.tar.xz" -C dav1d --strip-components=1
  cat > "$W/cross-mingw.txt" <<EOF
[binaries]
c = '$H-clang'
cpp = '$H-clang++'
ar = '$H-ar'
strip = '$H-strip'
windres = '$H-windres'
nasm = '$W/host/bin/nasm'
[built-in options]
c_args = ['$MAP']
[host_machine]
system = 'windows'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'
EOF
  (cd dav1d && meson setup build --cross-file "$W/cross-mingw.txt" --prefix="$P" --libdir=lib --buildtype=release \
     --default-library=static -Denable_tools=false -Denable_tests=false -Denable_examples=false >/dev/null \
     && ninja -C build >/dev/null && ninja -C build install >/dev/null)
fi

# pkg-config가 없는 환경용: dav1d만 알려 주는 작은 대체본(상대 경로 — 빌드 폴더 기준 ../prefix)
mkdir -p "$W/host/bin"
cat > "$W/host/bin/fakepc" <<'PC'
#!/bin/sh
case "$*" in
  *--exists*) case "$*" in *dav1d*) exit 0;; *) exit 1;; esac;;
  *--modversion*) echo DAV1D_VER;;
  *--cflags*) echo "-I../prefix/include";;
  *--libs*) echo "-L../prefix/lib -ldav1d";;
  *--version*) echo 0.29;;
  *) exit 1;;
esac
PC
sed -i "s/DAV1D_VER/$DAV1D_VER/" "$W/host/bin/fakepc"
chmod +x "$W/host/bin/fakepc"

# 컴파일러 감싸개: 소스 경로를 __FILE__에서 지운다(configure 줄에는 `--cc=wft-cc`만 남음)
cat > "$W/host/bin/wft-cc" <<CC
#!/bin/sh
exec $H-clang "\$@" "-ffile-prefix-map=$W/=" "-ffile-prefix-map=$W/bld/="
CC
chmod +x "$W/host/bin/wft-cc"

[ -d ffmpeg ] || { mkdir ffmpeg && tar xf "../dl/ffmpeg-$FFMPEG_VER.tar.xz" -C ffmpeg --strip-components=1; }

CONF=(--target-os=mingw32 --arch=x86_64 --cross-prefix=$H- --cc=wft-cc --enable-cross-compile
  --pkg-config=fakepc --extra-cflags=-I../prefix/include --extra-ldflags="-L../prefix/lib -static"
  --disable-everything --disable-doc --disable-debug --disable-network --disable-autodetect --disable-ffplay
  --enable-ffmpeg --enable-ffprobe --enable-small --enable-w32threads --enable-mediafoundation --enable-d3d11va --enable-zlib
  --enable-libmp3lame --enable-libdav1d --enable-decoder=libdav1d
  --enable-protocol=file,pipe
  --enable-demuxer=gif,mov,matroska,avi,hevc,h264,mp3,aac,wav,flac,ogg,image2,mpegts,mpegps,asf,flv,m4v,mjpeg
  --enable-muxer=mp4,ipod,mov,mp3,gif,image2,hevc,null,mjpeg
  --enable-decoder=hevc,h264,mpeg4,mpeg2video,mjpeg,vp8,vp9,av1,prores,png,gif,aac,aac_latm,mp3,mp3float,ac3,eac3,opus,vorbis,flac,alac,pcm_s16le,pcm_s24le,pcm_f32le,pcm_s16be,wmav2,wmv3,msmpeg4v3
  --enable-encoder=aac,libmp3lame,gif,png,mjpeg,h264_mf
  --enable-parser=hevc,h264,aac,mpegaudio,vp8,vp9,av1,opus,vorbis,flac,ac3,mjpeg,png
  --enable-bsf=hevc_mp4toannexb,h264_mp4toannexb,aac_adtstoasc,extract_extradata
  --enable-filter=xstack,hstack,vstack,overlay,scale,fps,split,palettegen,paletteuse,format,aformat,aresample,transpose,hflip,vflip,rotate,null,anull,crop,pad,setsar,copy,trim,atrim,setpts,asetpts,volume
  --enable-swscale --enable-swresample)

if [ ! -f "$W/out/bin/ffmpeg.exe" ] || [ "${FFMPEG_REBUILD:-}" = 1 ]; then
  rm -rf "$W/bld"; mkdir -p "$W/bld"; cd "$W/bld"
  t0=$(date +%s)
  # 상대 경로로 부른다(--prefix도 상대): configuration 문자열·__FILE__에 /home/... 경로가 남지 않게
  ../src/ffmpeg/configure --prefix=../out "${CONF[@]}" > configure.log 2>&1 || { tail -30 ffbuild/config.log; exit 1; }
  make -j"$(nproc)" > make.log 2>&1 && make install > install.log 2>&1
  echo "ffmpeg 빌드 $(( $(date +%s) - t0 ))초"
fi

cp "$W/out/bin/ffmpeg.exe" "$W/out/bin/ffprobe.exe" "$OUT/win-x64/bin/"
"$H-strip" "$OUT/win-x64/bin/"*.exe 2>/dev/null || true
for f in "ffmpeg-$FFMPEG_VER.tar.xz" "lame-$LAME_VER.tar.gz" "dav1d-$DAV1D_VER.tar.xz" "zlib-$ZLIB_VER.tar.gz"; do
  cp "$W/dl/$f" "$OUT/sources/"
done
{
  echo "ffmpeg $FFMPEG_VER — configure (빌드 폴더에서 ../src/ffmpeg/configure 로 실행, 툴체인 llvm-mingw $LLVM_MINGW)"
  printf '%q ' --prefix=../out "${CONF[@]}"; echo
  echo
  echo "함께 링크: LAME $LAME_VER (--disable-shared --enable-static --disable-frontend --disable-decoder --enable-nasm),"
  echo "           dav1d $DAV1D_VER (meson --default-library=static -Denable_tools=false -Denable_tests=false),"
  echo "           zlib $ZLIB_VER (win32/Makefile.gcc)"
  echo "빌드 스크립트: scripts/build-ffmpeg.sh"
} > "$OUT/sources/configure.txt"
(cd "$OUT/sources" && sha256sum ./*.tar.* > SHA256SUMS)
# 개인 경로가 남았는지 확인
if grep -a -c -E "/home/[a-z]|ffmpeg-build/" "$OUT/win-x64/bin/ffmpeg.exe" "$OUT/win-x64/bin/ffprobe.exe" | grep -v ':0$'; then
  echo "경고: exe 안에 로컬 경로 문자열이 있습니다" >&2
fi
ls -l "$OUT/win-x64/bin" "$OUT/sources"
