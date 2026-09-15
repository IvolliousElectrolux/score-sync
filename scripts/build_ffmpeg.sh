#!/usr/bin/env bash
# 给 GitHub Actions 编裁剪版 ffmpeg sidecar (GPL, 因 libx264). 不改上游源码.
# 不要在开发机上跑: 费时费电, 成品由 ffmpeg.yml 挂到 sidecars release.
set -euo pipefail

FFMPEG_TAG="${FFMPEG_TAG:-n7.1.1}"
X264_REF="${X264_REF:-stable}"
FFMPEG_URL="https://github.com/FFmpeg/FFmpeg/archive/refs/tags/${FFMPEG_TAG}.tar.gz"
X264_URL="https://github.com/mirror/x264/archive/refs/heads/${X264_REF}.tar.gz"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$ROOT/vendor"
WORK_DIR=""
ARCH=""
JOBS=""
DO_UPX=""
TARGET="native"

usage() {
  echo "usage: $0 [--out-dir DIR] [--work-dir DIR] [--arch arm64|x86_64] [--target native|mingw64] [--jobs N] [--skip-upx]" >&2
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --out-dir) OUT_DIR="$2"; shift 2 ;;
    --work-dir) WORK_DIR="$2"; shift 2 ;;
    --arch) ARCH="$2"; shift 2 ;;
    --target) TARGET="$2"; shift 2 ;;
    --jobs) JOBS="$2"; shift 2 ;;
    --skip-upx) DO_UPX=0; shift ;;
    -h|--help) usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

host_arch() {
  case "$(uname -m)" in
    arm64|aarch64) echo arm64 ;;
    x86_64|amd64) echo x86_64 ;;
    *) uname -m ;;
  esac
}

is_windows() {
  case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) return 0 ;;
    *) return 1 ;;
  esac
}

is_macos() {
  [ "$(uname -s)" = Darwin ]
}

if [ -z "$WORK_DIR" ]; then
  if [ -n "${RUNNER_TEMP:-}" ]; then
    WORK_DIR="$RUNNER_TEMP/score_sync-ffmpeg"
  else
    WORK_DIR="${HOME}/.cache/score_sync-ffmpeg"
  fi
fi

if ! command -v make >/dev/null 2>&1; then
  if command -v mingw32-make >/dev/null 2>&1; then
    make() { mingw32-make "$@"; }
  else
    echo "need make or mingw32-make" >&2
    exit 1
  fi
fi

for req in gcc tar curl nasm; do
  if ! command -v "$req" >/dev/null 2>&1; then
    echo "need $req on PATH" >&2
    exit 1
  fi
done

if [ -z "$ARCH" ]; then
  ARCH="$(host_arch)"
fi
if [ -z "$JOBS" ]; then
  n="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 8)"
  if [ "$n" -gt 12 ]; then
    JOBS=12
  else
    JOBS="$n"
  fi
fi

MINGW_CROSS=0
if [ "$TARGET" = mingw64 ]; then
  MINGW_CROSS=1
  ARCH=x86_64
  if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    echo "need x86_64-w64-mingw32-gcc (apt: gcc-mingw-w64-x86-64)" >&2
    exit 1
  fi
elif [ "$TARGET" != native ]; then
  echo "unknown --target $TARGET (native|mingw64)" >&2
  exit 2
fi

TARGET_WIN=0
EXE_SUFFIX=""
if is_windows || [ "$MINGW_CROSS" = 1 ]; then
  TARGET_WIN=1
  EXE_SUFFIX=".exe"
fi
if [ -z "$DO_UPX" ]; then
  if [ "$TARGET_WIN" = 1 ]; then
    DO_UPX=1
  else
    DO_UPX=0
  fi
fi

CROSS=0
if [ "$MINGW_CROSS" = 1 ]; then
  CROSS=1
elif is_macos && [ "$ARCH" != "$(host_arch)" ]; then
  CROSS=1
fi

PREFIX="$WORK_DIR/prefix"
SRC="$WORK_DIR/src"
mkdir -p "$OUT_DIR" "$PREFIX" "$SRC"

OUT_BIN="$OUT_DIR/ffmpeg${EXE_SUFFIX}"

echo "==> gcc: $(command -v gcc)"
gcc --version | head -n 1
echo "==> nasm: $(nasm -v)"
echo "==> arch=$ARCH jobs=$JOBS target=$TARGET cross=$CROSS upx=$DO_UPX"
echo "==> work=$WORK_DIR"
echo "==> out=$OUT_BIN"

fetch_tar() {
  local url="$1" dest="$2" marker="$3"
  if [ -f "$marker" ]; then
    echo "==> reuse $dest"
    return 0
  fi
  local tmp="$dest.tar.download"
  echo "==> download $url"
  curl -fL --retry 5 --retry-delay 2 -o "$tmp" "$url"
  mkdir -p "$dest"
  tar -xf "$tmp" -C "$dest" --strip-components=1
  rm -f "$tmp"
  : >"$marker"
}

fetch_tar "$X264_URL" "$SRC/x264" "$SRC/x264.ok"
fetch_tar "$FFMPEG_URL" "$SRC/ffmpeg" "$SRC/ffmpeg.ok"

X264_REV=""
if [ -f "$SRC/x264/version.sh" ]; then
  X264_REV="$("$SRC/x264/version.sh" 2>/dev/null | head -n 1 || true)"
fi

echo "==> build x264"
cd "$SRC/x264"
x264_cfg=(
  --prefix="$PREFIX"
  --enable-static
  --disable-cli
  --disable-opencl
  --enable-pic
  --enable-strip
)
if [ "$TARGET_WIN" = 1 ]; then
  x264_cfg+=(--extra-ldflags="-static")
fi
if [ "$MINGW_CROSS" = 1 ]; then
  x264_cfg+=(
    --host=x86_64-w64-mingw32
    --cross-prefix=x86_64-w64-mingw32-
  )
elif [ "$CROSS" = 1 ] && is_macos; then
  x264_cfg+=(
    --host="${ARCH}-apple-darwin"
    --cross-prefix=""
  )
  export CC="clang -arch $ARCH"
  export CFLAGS="-arch $ARCH"
  export LDFLAGS="-arch $ARCH"
fi
if [ ! -f "$PREFIX/lib/libx264.a" ]; then
  ./configure "${x264_cfg[@]}"
  make -j"$JOBS"
  make install
else
  echo "==> reuse libx264.a"
fi
unset CC CFLAGS LDFLAGS

export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"
X264_CFLAGS="-I$PREFIX/include"
X264_LIBS="-L$PREFIX/lib -lx264"
if command -v pkg-config >/dev/null 2>&1 && pkg-config --exists x264; then
  X264_CFLAGS="$(pkg-config --cflags x264)"
  X264_LIBS="$(pkg-config --static --libs x264 2>/dev/null || pkg-config --libs x264)"
fi

echo "==> build ffmpeg $FFMPEG_TAG"
cd "$SRC/ffmpeg"

# 覆盖现有导入格式的边界: wav (含 24bit/float/adpcm), mp3, flac,
# ogg (vorbis/opus/speex/flac), m4a/m4b/aac/mp4/mov (aac / HE-AAC / alac).
# 视频只编码 AVC (libx264), 不解码任何视频.
ENCODERS="libx264,aac,flac,pcm_s16le"
DECODERS="aac,aac_fixed,aac_latm,alac,mp3float,mp3,mp2,flac,vorbis,opus,speex"
DECODERS="$DECODERS,pcm_s16le,pcm_s16be,pcm_s24le,pcm_s24be,pcm_s32le,pcm_s32be"
DECODERS="$DECODERS,pcm_f32le,pcm_f32be,pcm_f64le,pcm_u8,pcm_alaw,pcm_mulaw"
DECODERS="$DECODERS,pcm_s16le_planar,pcm_s24le_planar,pcm_s32le_planar"
DECODERS="$DECODERS,adpcm_ima_wav,adpcm_ms,adpcm_ima_qt,rawvideo"
DEMUXERS="rawvideo,lavfi,mov,mp3,aac,flac,ogg,wav,w64,aiff,matroska"
MUXERS="mp4,mov,ipod,matroska,wav,s16le,flac,null,adts"
PARSERS="aac,aac_latm,mpegaudio,flac,vorbis,opus,h264,hevc"
BSFS="aac_adtstoasc,extract_extradata"
FILTERS="buffer,buffersink,abuffer,abuffersink,format,scale,aformat,aresample"
FILTERS="$FILTERS,atrim,setpts,asetpts,apad,concat,atempo,anull,anullsrc"

ff_cfg=(
  --prefix="$PREFIX"
  --enable-gpl
  --enable-libx264
  --enable-static
  --disable-shared
  --disable-everything
  --disable-autodetect
  --disable-network
  --disable-avdevice
  --disable-postproc
  --disable-doc
  --disable-htmlpages
  --disable-manpages
  --disable-podpages
  --disable-txtpages
  --disable-ffplay
  --disable-ffprobe
  --disable-debug
  --disable-iconv
  --enable-small
  --enable-protocol=file,pipe
  --enable-encoder="$ENCODERS"
  --enable-decoder="$DECODERS"
  --enable-demuxer="$DEMUXERS"
  --enable-muxer="$MUXERS"
  --enable-parser="$PARSERS"
  --enable-bsf="$BSFS"
  --enable-filter="$FILTERS"
  --extra-cflags="$X264_CFLAGS"
  --extra-ldflags="-L$PREFIX/lib"
  --extra-libs="$X264_LIBS"
)
if [ "$TARGET_WIN" = 1 ]; then
  ff_cfg+=(
    --target-os=mingw32
    --arch=x86_64
    --extra-ldflags="-L$PREFIX/lib -static -static-libgcc"
    --extra-libs="$X264_LIBS -lwinpthread"
  )
fi
if [ "$MINGW_CROSS" = 1 ]; then
  ff_cfg+=(
    --enable-cross-compile
    --cross-prefix=x86_64-w64-mingw32-
    --cc=x86_64-w64-mingw32-gcc
    --cxx=x86_64-w64-mingw32-g++
    --nm=x86_64-w64-mingw32-nm
    --ar=x86_64-w64-mingw32-ar
    --ranlib=x86_64-w64-mingw32-ranlib
    --strip=x86_64-w64-mingw32-strip
  )
fi
if is_macos; then
  ff_cfg+=(
    --disable-videotoolbox
    --disable-audiotoolbox
    --disable-appkit
    --extra-ldflags="-L$PREFIX/lib -Wl,-dead_strip"
  )
  if [ "$CROSS" = 1 ]; then
    ff_cfg+=(
      --enable-cross-compile
      --arch="$ARCH"
      --target-os=darwin
      --cc="clang -arch $ARCH"
      --cxx="clang++ -arch $ARCH"
      --extra-cflags="$X264_CFLAGS -arch $ARCH"
      --extra-ldflags="-L$PREFIX/lib -arch $ARCH -Wl,-dead_strip"
    )
  fi
fi

./configure "${ff_cfg[@]}"
make -j"$JOBS"
make install

BUILT="$PREFIX/bin/ffmpeg${EXE_SUFFIX}"
if [ ! -f "$BUILT" ]; then
  echo "ffmpeg binary missing: $BUILT" >&2
  exit 1
fi

STRIP_BIN="strip"
if [ "$MINGW_CROSS" = 1 ]; then
  STRIP_BIN="x86_64-w64-mingw32-strip"
fi
if command -v "$STRIP_BIN" >/dev/null 2>&1; then
  "$STRIP_BIN" -s "$BUILT" 2>/dev/null || "$STRIP_BIN" "$BUILT" || true
fi

if [ "$DO_UPX" = 1 ]; then
  if command -v upx >/dev/null 2>&1; then
    echo "==> upx"
    upx --best --lzma "$BUILT" || upx --best "$BUILT"
  else
    echo "upx not on PATH, skip" >&2
  fi
fi

cp -f "$BUILT" "$OUT_BIN"
if [ "$TARGET_WIN" != 1 ]; then
  chmod +x "$OUT_BIN"
fi

{
  echo "This sidecar ffmpeg is GPL (libx264 + FFmpeg --enable-gpl)."
  echo "score_sync stays MIT and only execs this binary as a separate process."
  echo
  echo "No upstream source was patched; components are selected by configure."
  echo "Recipe: scripts/build_ffmpeg.sh  (GitHub Actions: .github/workflows/ffmpeg.yml)"
  echo
  echo "FFmpeg ${FFMPEG_TAG}  ${FFMPEG_URL}"
  echo "x264 ${X264_REF}  ${X264_URL}"
  if [ -n "$X264_REV" ]; then
    echo "x264 version: $X264_REV"
  fi
  echo
  echo "Corresponding source is the public tarballs above plus this script."
  echo
} >"$OUT_DIR/ffmpeg-SOURCE.txt"

{
  echo "===== FFmpeg ====="
  cat "$SRC/ffmpeg/COPYING.GPLv2" 2>/dev/null || cat "$SRC/ffmpeg/COPYING.GPLv3"
  echo
  echo "===== x264 ====="
  cat "$SRC/x264/COPYING"
} >"$OUT_DIR/ffmpeg-COPYING.txt"

if [ "$CROSS" = 1 ]; then
  echo "==> skip smoke test (cross-compiled)"
else
  echo "==> smoke test"
  "$OUT_BIN" -hide_banner -version | head -n 2
  "$OUT_BIN" -hide_banner -encoders 2>/dev/null | grep -E "libx264|aac|flac" || true
  "$OUT_BIN" -hide_banner -decoders 2>/dev/null | grep -E "aac|mp3|flac|vorbis|alac|opus" || true
  null_out="/dev/null"
  if is_windows; then
    null_out="NUL"
  fi
  "$OUT_BIN" -y -hide_banner -loglevel error \
    -f lavfi -i anullsrc=r=44100:cl=stereo -t 0.15 -c:a aac -f mp4 "$null_out"
  "$OUT_BIN" -y -hide_banner -loglevel error \
    -f rawvideo -pix_fmt rgba -s 64x64 -framerate 2 -i - \
    -frames:v 2 -vf format=yuv420p -c:v libx264 -preset ultrafast -an -f mp4 "$null_out" \
    </dev/zero
fi

bytes="$(wc -c <"$OUT_BIN" | tr -d ' ')"
echo "==> done $OUT_BIN ($bytes bytes)"
