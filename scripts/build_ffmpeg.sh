#!/usr/bin/env bash
# 给 GitHub Actions 编裁剪版 ffmpeg sidecar (GPL, 因 libx264). 不改上游源码.
# 不要在开发机上跑: 费时费电, 成品由 ffmpeg.yml 挂到 sidecars release.
# 能力清单 (写功能时对照): README.md 「裁剪版 ffmpeg」; 改能力只改下面 ENCODERS 等变量.
set -euo pipefail

FFMPEG_TAG="${FFMPEG_TAG:-n7.1.1}"
X264_REF="${X264_REF:-stable}"
FFMPEG_URL="https://github.com/FFmpeg/FFmpeg/archive/refs/tags/${FFMPEG_TAG}.tar.gz"
X264_URL="https://github.com/mirror/x264/archive/refs/heads/${X264_REF}.tar.gz"
# 硬件编码器只加头文件或静态 dispatcher, 运行时再加载本机驱动, 不把驱动打进 sidecar.
NVENC_TAG="${NVENC_TAG:-n12.2.72.0}"
AMF_TAG="${AMF_TAG:-v1.4.35}"
LIBVPL_TAG="${LIBVPL_TAG:-v2.14.0}"
NVENC_URL="https://github.com/FFmpeg/nv-codec-headers/archive/refs/tags/${NVENC_TAG}.tar.gz"
AMF_URL="https://github.com/GPUOpen-LibrariesAndSDKs/AMF/archive/refs/tags/${AMF_TAG}.tar.gz"
LIBVPL_URL="https://github.com/intel/libvpl/archive/refs/tags/${LIBVPL_TAG}.tar.gz"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$ROOT/vendor"
WORK_DIR=""
ARCH=""
JOBS=""
DO_UPX=""
TARGET="native"

usage() {
  echo "usage: $0 [--out-dir DIR] [--work-dir DIR] [--arch arm64|x86_64] [--target native|mingw64|msvc] [--jobs N] [--skip-upx]" >&2
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
MSVC=0
if [ "$TARGET" = mingw64 ]; then
  MINGW_CROSS=1
  ARCH=x86_64
  if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    echo "need x86_64-w64-mingw32-gcc (apt: gcc-mingw-w64-x86-64)" >&2
    exit 1
  fi
elif [ "$TARGET" = msvc ]; then
  # fasterthanlime gist: POSIX 壳 + GNU make, 编译器用 cl, 成品无 msys dll.
  # https://gist.github.com/fasterthanlime/b674346115e88b762d76dac02fef6bd3
  MSVC=1
  ARCH=x86_64
  if ! command -v cl >/dev/null 2>&1 && ! command -v cl.exe >/dev/null 2>&1; then
    echo "need cl.exe (MSVC Developer Command Prompt / ilammy/msvc-dev-cmd)" >&2
    exit 1
  fi
elif [ "$TARGET" != native ]; then
  echo "unknown --target $TARGET (native|mingw64|msvc)" >&2
  exit 2
fi

if [ "$MSVC" != 1 ]; then
  for req in gcc tar curl nasm; do
    if ! command -v "$req" >/dev/null 2>&1; then
      echo "need $req on PATH" >&2
      exit 1
    fi
  done
else
  for req in tar curl nasm; do
    if ! command -v "$req" >/dev/null 2>&1; then
      echo "need $req on PATH" >&2
      exit 1
    fi
  done
fi

TARGET_WIN=0
EXE_SUFFIX=""
if is_windows || [ "$MINGW_CROSS" = 1 ] || [ "$MSVC" = 1 ]; then
  TARGET_WIN=1
  EXE_SUFFIX=".exe"
fi
WANT_NVENC=0
WANT_AMF=0
WANT_QSV=0
WANT_MF=0
WANT_D3D11=0
WANT_VT=0
if [ -z "$DO_UPX" ]; then
  # Windows / Linux ELF 可以 UPX. macOS Mach-O 不压, 免得 Gatekeeper 拒签.
  if [ "$TARGET_WIN" = 1 ]; then
    DO_UPX=1
  elif [ "$(uname -s)" = Linux ]; then
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
OUT_DIR="$(cd "$OUT_DIR" && pwd)"
WORK_DIR="$(cd "$WORK_DIR" && pwd)"
PREFIX="$WORK_DIR/prefix"
SRC="$WORK_DIR/src"

OUT_BIN="$OUT_DIR/ffmpeg${EXE_SUFFIX}"

if [ "$MSVC" = 1 ]; then
  echo "==> cl: $(command -v cl || command -v cl.exe)"
  cl 2>&1 | head -n 1 || true
else
  echo "==> gcc: $(command -v gcc)"
  gcc --version | head -n 1
fi
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

# 源码 cache 跨平台共用; Windows 编完留下的 .o/.obj/.depend 不能给 Linux/macOS 链.
wipe_build_tree() {
  local dir="$1"
  [ -d "$dir" ] || return 0
  echo "==> wipe stale objects in $dir"
  if [ -f "$dir/Makefile" ] || [ -f "$dir/ffbuild/config.mak" ] || [ -f "$dir/config.mak" ]; then
    make -C "$dir" distclean >/dev/null 2>&1 || make -C "$dir" clean >/dev/null 2>&1 || true
  fi
  find "$dir" -type f \( \
    -name '*.o' -o -name '*.obj' -o -name '*.a' -o -name '*.lib' \
    -o -name '*.d' -o -name '*.def' -o -name '.depend' -o -name '*.exe' \
    -o -name 'ffmpeg' -o -name 'ffmpeg_g' -o -name 'x264' \
  \) -delete 2>/dev/null || true
}

# nv-codec-headers 只有头文件. 运行时由驱动提供 nvEncodeAPI.
install_nvenc_headers() {
  fetch_tar "$NVENC_URL" "$SRC/nv-codec-headers" "$SRC/nv-codec-headers.ok"
  echo "==> install nv-codec-headers $NVENC_TAG"
  make -C "$SRC/nv-codec-headers" PREFIX="$PREFIX" install
}

# AMF 只有头文件. 运行时 LoadLibrary 驱动里的 amfrt.
install_amf_headers() {
  fetch_tar "$AMF_URL" "$SRC/amf" "$SRC/amf.ok"
  local inc="$SRC/amf/amf/public/include"
  if [ ! -f "$inc/core/Version.h" ]; then
    echo "AMF headers missing: $inc" >&2
    exit 1
  fi
  echo "==> install AMF headers $AMF_TAG"
  rm -rf "$PREFIX/include/AMF"
  mkdir -p "$PREFIX/include/AMF"
  cp -a "$inc/." "$PREFIX/include/AMF/"
}

# oneVPL dispatcher 静态链进 ffmpeg. 运行时再找 Intel 驱动里的实现.
# 不改 libvpl 源码. MSVC 的上游 pkg-config 不写 ole32/advapi32, 装好后只改 .pc.
install_libvpl() {
  if ! command -v cmake >/dev/null 2>&1; then
    echo "need cmake on PATH" >&2
    exit 1
  fi
  fetch_tar "$LIBVPL_URL" "$SRC/libvpl" "$SRC/libvpl.ok"
  echo "==> build libvpl $LIBVPL_TAG"
  rm -rf "$SRC/libvpl/build"
  local src_arg="$SRC/libvpl"
  local prefix_arg="$PREFIX"
  local build_arg="$SRC/libvpl/build"
  local -a gen=(-G "Unix Makefiles")
  local -a msvc_cc=()
  if [ "$MSVC" = 1 ]; then
    src_arg="$(cygpath -w "$SRC/libvpl")"
    prefix_arg="$(cygpath -w "$PREFIX")"
    build_arg="$(cygpath -w "$SRC/libvpl/build")"
    local cc
    cc="$(command -v cl.exe || command -v cl)"
    cc="$(cygpath -w "$cc")"
    msvc_cc=(-DCMAKE_C_COMPILER="$cc" -DCMAKE_CXX_COMPILER="$cc")
    if command -v ninja >/dev/null 2>&1 || command -v ninja.exe >/dev/null 2>&1; then
      gen=(-G Ninja)
    else
      gen=(-G "NMake Makefiles")
    fi
  fi
  cmake -S "$src_arg" -B "$build_arg" "${gen[@]}" \
    -DCMAKE_BUILD_TYPE=Release \
    -DBUILD_SHARED_LIBS=OFF \
    -DBUILD_TESTS=OFF \
    -DBUILD_EXAMPLES=OFF \
    -DINSTALL_DEV=ON \
    -DINSTALL_LIB=ON \
    -DCMAKE_INSTALL_PREFIX="$prefix_arg" \
    -DCMAKE_INSTALL_LIBDIR=lib \
    -Duse_control_flow_guard=OFF \
    -Duse_qspectre=OFF \
    "${msvc_cc[@]}"
  if [ "$MSVC" = 1 ]; then
    cmake --build "$build_arg"
    cmake --install "$build_arg"
  else
    cmake --build "$build_arg" -j"$JOBS"
    cmake --install "$build_arg"
  fi
  rm -rf "$SRC/libvpl/build"
  local pc="$PREFIX/lib/pkgconfig/vpl.pc"
  if [ ! -f "$pc" ]; then
    echo "vpl.pc missing: $pc" >&2
    exit 1
  fi
  if [ "$MSVC" = 1 ]; then
    local tmp="$pc.tmp"
    awk '
      /^Libs:/ { print "Libs: -L${libdir} -lvpl -ladvapi32 -lole32 -luuid -lgdi32 -lmsvcprt"; next }
      { print }
    ' "$pc" >"$tmp"
    mv "$tmp" "$pc"
  fi
  echo "==> vpl $(${PKG_CONFIG:-pkg-config} --modversion vpl)"
  "${PKG_CONFIG:-pkg-config}" --libs vpl
}

install_hw_deps() {
  if [ "$TARGET_WIN" = 1 ]; then
    WANT_NVENC=1
    WANT_AMF=1
    if [ "$MSVC" = 1 ]; then
      WANT_QSV=1
      WANT_MF=1
      WANT_D3D11=1
    fi
  elif is_macos; then
    WANT_VT=1
  elif [ "$(uname -s)" = Linux ] && [ "$MINGW_CROSS" != 1 ]; then
    WANT_NVENC=1
    WANT_AMF=1
    WANT_QSV=1
  fi
  echo "==> hw nvenc=$WANT_NVENC amf=$WANT_AMF qsv=$WANT_QSV mf=$WANT_MF d3d11=$WANT_D3D11 vt=$WANT_VT"
  if [ "$WANT_NVENC" = 1 ]; then
    install_nvenc_headers
  fi
  if [ "$WANT_AMF" = 1 ]; then
    install_amf_headers
  fi
  if [ "$WANT_QSV" = 1 ]; then
    install_libvpl
  fi
}

fetch_tar "$X264_URL" "$SRC/x264" "$SRC/x264.ok"
fetch_tar "$FFMPEG_URL" "$SRC/ffmpeg" "$SRC/ffmpeg.ok"
wipe_build_tree "$SRC/x264"
wipe_build_tree "$SRC/ffmpeg"

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
if [ "$TARGET_WIN" = 1 ] && [ "$MSVC" != 1 ]; then
  x264_cfg+=(--extra-ldflags="-static")
fi
if [ "$MSVC" = 1 ]; then
  export CC=cl
  x264_cfg+=(--host=x86_64-w64-mingw32)
elif [ "$MINGW_CROSS" = 1 ]; then
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
if [ ! -f "$PREFIX/lib/libx264.a" ] && [ ! -f "$PREFIX/lib/libx264.lib" ]; then
  ./configure "${x264_cfg[@]}"
  make -j"$JOBS"
  make install
else
  echo "==> reuse libx264.a"
fi
unset CC CFLAGS LDFLAGS

export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"
if [ "$MSVC" = 1 ] && [ -x /usr/bin/pkg-config ]; then
  export PKG_CONFIG=/usr/bin/pkg-config
fi
X264_CFLAGS="-I$PREFIX/include"
X264_LIBS="-L$PREFIX/lib -lx264"
if command -v "${PKG_CONFIG:-pkg-config}" >/dev/null 2>&1 && "${PKG_CONFIG:-pkg-config}" --exists x264; then
  X264_CFLAGS="$("${PKG_CONFIG:-pkg-config}" --cflags x264)"
  X264_LIBS="$("${PKG_CONFIG:-pkg-config}" --static --libs x264 2>/dev/null || "${PKG_CONFIG:-pkg-config}" --libs x264)"
fi
if [ "$MSVC" = 1 ]; then
  PREFIX_WIN="$(cygpath -w "$PREFIX")"
  X264_CFLAGS="//I${PREFIX_WIN}\\include"
  X264_LIBS=""
  echo "==> pkg-config: ${PKG_CONFIG:-pkg-config}"
  "${PKG_CONFIG:-pkg-config}" --modversion x264 || true
  echo "==> x264.pc prefix=$(sed -n 's/^prefix=//p' "$PREFIX/lib/pkgconfig/x264.pc" 2>/dev/null || true)"
fi

install_hw_deps

echo "==> build ffmpeg $FFMPEG_TAG"
cd "$SRC/ffmpeg"

# 覆盖现有导入格式的边界: wav (含 24bit/float/adpcm), mp3, flac,
# ogg (vorbis/opus/speex/flac), m4a/m4b/aac/mp4/mov (aac / HE-AAC / alac).
# 视频只编码 AVC, 不解码任何视频. 硬件编码器按平台加, 运行时再探测驱动.
ENCODERS="libx264"
if [ "$WANT_QSV" = 1 ]; then ENCODERS="$ENCODERS,h264_qsv"; fi
if [ "$WANT_NVENC" = 1 ]; then ENCODERS="$ENCODERS,h264_nvenc"; fi
if [ "$WANT_AMF" = 1 ]; then ENCODERS="$ENCODERS,h264_amf"; fi
if [ "$WANT_MF" = 1 ]; then ENCODERS="$ENCODERS,h264_mf"; fi
if [ "$WANT_VT" = 1 ]; then ENCODERS="$ENCODERS,h264_videotoolbox"; fi
ENCODERS="$ENCODERS,aac,flac,pcm_s16le"
DECODERS="aac,aac_fixed,aac_latm,alac,mp3float,mp3,mp2,flac,vorbis,opus,speex"
DECODERS="$DECODERS,pcm_s16le,pcm_s16be,pcm_s24le,pcm_s24be,pcm_s32le,pcm_s32be"
DECODERS="$DECODERS,pcm_f32le,pcm_f32be,pcm_f64le,pcm_u8,pcm_alaw,pcm_mulaw"
DECODERS="$DECODERS,pcm_s16le_planar,pcm_s24le_planar,pcm_s32le_planar"
DECODERS="$DECODERS,adpcm_ima_wav,adpcm_ms,adpcm_ima_qt,rawvideo"
DEMUXERS="rawvideo,lavfi,mov,mp3,aac,flac,ogg,wav,w64,aiff,matroska"
# CLI 是 `-f s16le`, configure 组件名是 `pcm_s16le` (见 libavformat/Makefile).
# 写成 s16le 会被 --disable-everything 静默丢掉, 预览管道就没有 raw PCM muxer.
MUXERS="mp4,mov,ipod,matroska,wav,pcm_s16le,flac,null,adts"
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
  --enable-avdevice
  --enable-indev=lavfi
  --enable-protocol=file,pipe,fd
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
if [ "$MSVC" = 1 ]; then
  PREFIX_WIN="$(cygpath -w "$PREFIX")"
  ff_cfg+=(
    --toolchain=msvc
    --target-os=win64
    --arch=x86_64
    --enable-w32threads
    --pkg-config="${PKG_CONFIG:-pkg-config}"
    --extra-cflags="//I${PREFIX_WIN}\\include"
    --extra-ldflags="//LIBPATH:${PREFIX_WIN}\\lib"
  )
elif [ "$MINGW_CROSS" = 1 ]; then
  ff_cfg+=(
    --target-os=mingw32
    --arch=x86_64
    --extra-ldflags="-L$PREFIX/lib -static -static-libgcc"
    --extra-libs="$X264_LIBS -lwinpthread"
    --enable-cross-compile
    --cross-prefix=x86_64-w64-mingw32-
    --pkg-config=pkg-config
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
    --enable-videotoolbox
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
if [ "$(uname -s)" = Linux ] && [ "$MINGW_CROSS" != 1 ]; then
  ff_cfg+=(
    --enable-pic
    --extra-ldflags="-L$PREFIX/lib -Wl,--gc-sections"
  )
  if command -v "${PKG_CONFIG:-pkg-config}" >/dev/null 2>&1 && "${PKG_CONFIG:-pkg-config}" --exists zlib; then
    ff_cfg+=(--enable-zlib)
  fi
fi
if [ "$WANT_NVENC" = 1 ]; then
  ff_cfg+=(--enable-ffnvcodec --enable-nvenc)
fi
if [ "$WANT_AMF" = 1 ]; then
  ff_cfg+=(--enable-amf)
fi
if [ "$WANT_QSV" = 1 ]; then
  ff_cfg+=(--enable-libvpl)
fi
if [ "$WANT_MF" = 1 ]; then
  ff_cfg+=(--enable-mediafoundation)
fi
if [ "$WANT_D3D11" = 1 ]; then
  ff_cfg+=(--enable-d3d11va)
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
if [ "$MSVC" != 1 ] && command -v "$STRIP_BIN" >/dev/null 2>&1; then
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
  if [ "$WANT_NVENC" = 1 ]; then
    echo "nv-codec-headers ${NVENC_TAG}  ${NVENC_URL}"
  fi
  if [ "$WANT_AMF" = 1 ]; then
    echo "AMF ${AMF_TAG}  ${AMF_URL}"
  fi
  if [ "$WANT_QSV" = 1 ]; then
    echo "libvpl ${LIBVPL_TAG}  ${LIBVPL_URL}"
  fi
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
  if [ -f "$SRC/libvpl/LICENSE" ]; then
    echo
    echo "===== libvpl (MIT) ====="
    cat "$SRC/libvpl/LICENSE"
  fi
  if [ -f "$SRC/nv-codec-headers/LICENSE" ]; then
    echo
    echo "===== nv-codec-headers ====="
    cat "$SRC/nv-codec-headers/LICENSE"
  fi
} >"$OUT_DIR/ffmpeg-COPYING.txt"

if [ "$CROSS" = 1 ]; then
  echo "==> skip smoke test (cross-compiled)"
else
  echo "==> smoke test"
  "$OUT_BIN" -hide_banner -version | head -n 2
  enc_list="$("$OUT_BIN" -hide_banner -encoders 2>/dev/null || true)"
  printf '%s\n' "$enc_list" | grep -E "libx264|aac|flac" || true
  "$OUT_BIN" -hide_banner -decoders 2>/dev/null | grep -E "aac|mp3|flac|vorbis|alac|opus" || true
  need_encoder() {
    if ! printf '%s\n' "$enc_list" | grep -F -q " $1"; then
      echo "missing encoder: $1" >&2
      printf '%s\n' "$enc_list" >&2
      exit 1
    fi
  }
  need_encoder libx264
  if [ "$WANT_QSV" = 1 ]; then need_encoder h264_qsv; fi
  if [ "$WANT_NVENC" = 1 ]; then need_encoder h264_nvenc; fi
  if [ "$WANT_AMF" = 1 ]; then need_encoder h264_amf; fi
  if [ "$WANT_MF" = 1 ]; then need_encoder h264_mf; fi
  if [ "$WANT_VT" = 1 ]; then need_encoder h264_videotoolbox; fi
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
  if [ "$WANT_VT" = 1 ]; then
    "$OUT_BIN" -y -hide_banner -loglevel error \
      -f rawvideo -pix_fmt rgba -s 64x64 -framerate 2 -i - \
      -frames:v 2 -vf format=yuv420p -c:v h264_videotoolbox -profile:v high -q:v 70 \
      -an -f mp4 "$null_out" \
      </dev/zero
  fi
fi

bytes="$(wc -c <"$OUT_BIN" | tr -d ' ')"
echo "==> done $OUT_BIN ($bytes bytes)"
