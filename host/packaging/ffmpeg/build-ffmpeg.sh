#!/usr/bin/env bash
# Builds keyglass-ffmpeg and keyglass-ffprobe, the FFmpeg command-line tools
# bundled with Keyglass Studio for importing media.
#
# The build is self-contained: FFmpeg and x264 come from pinned, verified
# sources, autodetection is off so nothing on the build machine is linked by
# accident, and the only enabled components are the ones the daemon's import
# pipeline uses (decode common video, scale/retime/reverse, cut into segments
# and join them again, encode H.264, write MP4 and JPEG thumbnails, read
# durations). Both tools come from the same configured tree. The result is
# GPL-2.0-or-later because it includes x264. The tools ship as separate
# executables that Keyglass runs as child processes.
#
# Usage:
#   build-ffmpeg.sh --target <triple> --out <dir> [--work <dir>] [--jobs <n>]
#   build-ffmpeg.sh --source-archive <file.tar.xz> [--work <dir>]
#
# Targets:
#   aarch64-apple-darwin, x86_64-apple-darwin   (macOS 13+, clang; x86_64 is
#                                                 cross-compiled on Apple silicon)
#   x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu   (native gcc)
#   x86_64-pc-windows-msvc   (MinGW-w64 in an MSYS2 UCRT64 shell; the
#                             executable has no MinGW runtime dependencies)
#
# Outputs, for --target:
#   <out>/keyglass-ffmpeg[.exe]
#   <out>/keyglass-ffprobe[.exe]
#   <out>/keyglass-ffmpeg-<target>.buildinfo.txt   configure lines and toolchain
#   <out>/keyglass-ffmpeg-COPYING.GPLv2.txt        the license both tools are under
#
# --source-archive writes the complete corresponding source: the verified
# upstream FFmpeg tarball, the x264 source at the pinned commit, and this
# script. Honors SOURCE_DATE_EPOCH for a reproducible archive.

set -euo pipefail

FFMPEG_VERSION="8.1.3"
FFMPEG_SHA256="7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3"
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz"
X264_REPO="https://code.videolan.org/videolan/x264.git"
X264_COMMIT="b35605ace3ddf7c1a5d67a2eb553f034aef41d55"
MACOS_MIN="13.0"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target=""
out_dir=""
work_dir="${PWD}/build/ffmpeg"
source_archive=""
jobs=""

die() { echo "build-ffmpeg: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --target) target="$2"; shift 2 ;;
        --out) out_dir="$2"; shift 2 ;;
        --work) work_dir="$2"; shift 2 ;;
        --jobs) jobs="$2"; shift 2 ;;
        --source-archive) source_archive="$2"; shift 2 ;;
        -h|--help) sed -n '2,/^$/p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
done

if [ -z "$jobs" ]; then
    jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
fi

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

mkdir -p "$work_dir"
work_dir="$(cd "$work_dir" && pwd)"
ffmpeg_tarball="${work_dir}/ffmpeg-${FFMPEG_VERSION}.tar.xz"
x264_src="${work_dir}/x264-${X264_COMMIT}"

fetch_sources() {
    if [ ! -f "$ffmpeg_tarball" ] || [ "$(sha256_of "$ffmpeg_tarball")" != "$FFMPEG_SHA256" ]; then
        rm -f "$ffmpeg_tarball"
        curl --fail --location --silent --show-error --retry 3 -o "${ffmpeg_tarball}.part" "$FFMPEG_URL"
        mv "${ffmpeg_tarball}.part" "$ffmpeg_tarball"
    fi
    actual="$(sha256_of "$ffmpeg_tarball")"
    [ "$actual" = "$FFMPEG_SHA256" ] || die "FFmpeg tarball sha256 mismatch: expected ${FFMPEG_SHA256}, got ${actual}"

    # Git verifies the content against the commit ID, which makes the commit
    # the checksum for x264.
    if [ ! -d "${x264_src}/.git" ]; then
        rm -rf "$x264_src"
        git init --quiet "$x264_src"
        git -C "$x264_src" fetch --quiet --depth 1 "$X264_REPO" "$X264_COMMIT"
        git -C "$x264_src" -c advice.detachedHead=false checkout --quiet FETCH_HEAD
    fi
    head="$(git -C "$x264_src" rev-parse HEAD)"
    [ "$head" = "$X264_COMMIT" ] || die "x264 checkout is ${head}, expected ${X264_COMMIT}"
}

write_source_archive() {
    local dest="$1" stage name epoch
    name="keyglass-ffmpeg-${FFMPEG_VERSION}-source"
    stage="${work_dir}/${name}"
    epoch="${SOURCE_DATE_EPOCH:-0}"
    rm -rf "$stage"
    mkdir -p "$stage"
    cp "$ffmpeg_tarball" "$stage/"
    git -C "$x264_src" archive --format=tar --prefix="x264-${X264_COMMIT}/" HEAD > "${stage}/x264-${X264_COMMIT}.tar"
    cp "${script_dir}/build-ffmpeg.sh" "$stage/"
    cat > "${stage}/README.txt" <<EOF
keyglass-ffmpeg ${FFMPEG_VERSION} - complete corresponding source

keyglass-ffmpeg and keyglass-ffprobe are the ffmpeg and ffprobe tools of
FFmpeg ${FFMPEG_VERSION} built with x264. They are distributed with Keyglass
Studio as separate executables under the GNU General Public License,
version 2 or later (keyglass-ffmpeg-COPYING.GPLv2.txt).

  ffmpeg-${FFMPEG_VERSION}.tar.xz     upstream release, sha256 ${FFMPEG_SHA256}
  x264-${X264_COMMIT}.tar   ${X264_REPO} at commit ${X264_COMMIT}
  build-ffmpeg.sh            the script that configures and builds both

Rebuild for a target with:
  ./build-ffmpeg.sh --target <triple> --out <dir>
The configure lines used for each published build are in the
keyglass-ffmpeg-<target>.buildinfo.txt files published with the same release.
EOF
    mkdir -p "$(dirname "$dest")"
    if tar --version 2>/dev/null | grep -q 'GNU tar'; then
        tar --sort=name --mtime="@${epoch}" --owner=0 --group=0 --numeric-owner \
            -C "$work_dir" -cf - "$name" | xz -9e -T1 > "$dest"
    else
        # bsdtar has no --sort; the file set is small and listed explicitly.
        (cd "$work_dir" && find "$name" -print | LC_ALL=C sort > "${work_dir}/files.txt")
        tar -C "$work_dir" --uid 0 --gid 0 --uname root --gname root -n -T "${work_dir}/files.txt" -cf - | xz -9e -T1 > "$dest"
    fi
    echo "wrote ${dest}"
}

if [ -n "$source_archive" ]; then
    fetch_sources
    write_source_archive "$source_archive"
    exit 0
fi

[ -n "$target" ] || die "--target is required"
[ -n "$out_dir" ] || die "--out is required"
mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"

# Compiler and linker flags come only from this script.
unset CFLAGS CXXFLAGS CPPFLAGS LDFLAGS LIBRARY_PATH CPATH

exe_suffix=""
x264_flags=(--enable-static --disable-cli --enable-pic --disable-opencl --disable-avs
            --disable-swscale --disable-lavf --disable-ffms --disable-gpac --disable-lsmash
            --bit-depth=8)
ffmpeg_flags=()
cc="${CC:-cc}"
extra_cflags=""
extra_ldflags=""

case "$target" in
    aarch64-apple-darwin|x86_64-apple-darwin)
        arch="${target%%-*}"
        cc="clang -arch ${arch/aarch64/arm64}"
        extra_cflags="-mmacosx-version-min=${MACOS_MIN}"
        extra_ldflags="-mmacosx-version-min=${MACOS_MIN}"
        x264_flags+=(--host="${target}")
        ffmpeg_flags+=(--target-os=darwin --arch="$arch" --enable-pthreads
                       --enable-videotoolbox --enable-encoder=h264_videotoolbox)
        if [ "$(uname -m)" != "${arch/aarch64/arm64}" ]; then
            ffmpeg_flags+=(--enable-cross-compile)
        fi
        ;;
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu)
        arch="${target%%-*}"
        [ "$(uname -m)" = "$arch" ] || die "${target} is built natively; this machine is $(uname -m)"
        extra_ldflags="-static-libgcc"
        ffmpeg_flags+=(--enable-pthreads)
        ;;
    x86_64-pc-windows-msvc)
        case "${MSYSTEM:-}" in
            UCRT64|MINGW64) ;;
            *) die "${target} is built in an MSYS2 UCRT64 shell" ;;
        esac
        exe_suffix=".exe"
        cc="gcc"
        extra_ldflags="-static"
        x264_flags+=(--host=x86_64-w64-mingw32)
        ffmpeg_flags+=(--target-os=mingw32 --arch=x86_64 --enable-w32threads)
        ;;
    *)
        die "unsupported target: ${target}"
        ;;
esac

fetch_sources

build_root="${work_dir}/${target}"
prefix="${build_root}/prefix"
rm -rf "$build_root"
mkdir -p "$build_root" "$prefix"

echo "build-ffmpeg: x264 ${X264_COMMIT} for ${target}"
cp -R "$x264_src" "${build_root}/x264"
x264_configure=(./configure --prefix="$prefix" "${x264_flags[@]}"
                --extra-cflags="$extra_cflags" --extra-ldflags="$extra_ldflags")
(
    cd "${build_root}/x264"
    CC="$cc" "${x264_configure[@]}"
    make -j"$jobs"
    make install-lib-static
)

echo "build-ffmpeg: FFmpeg ${FFMPEG_VERSION} for ${target}"
tar -xJf "$ffmpeg_tarball" -C "$build_root"
# Component lists are comma-separated in configure syntax.
# shellcheck disable=SC2054
ffmpeg_configure=(./configure
    --prefix="$prefix"
    --cc="$cc"
    --pkg-config-flags=--static
    --extra-cflags="-I${prefix}/include ${extra_cflags}"
    --extra-ldflags="-L${prefix}/lib ${extra_ldflags}"
    --enable-gpl
    --enable-static --disable-shared
    --disable-autodetect
    --disable-everything
    --disable-doc --disable-debug --disable-ffplay
    --disable-network
    --enable-ffmpeg --enable-ffprobe
    --enable-avdevice --enable-avfilter --enable-swscale
    --enable-libx264
    --enable-protocol=file,pipe
    --enable-demuxer=mov,matroska,avi,mpegts,flv,gif,image2,mjpeg,h264,hevc,m4v,concat
    --enable-decoder=h264,hevc,vp8,vp9,mpeg4,mpeg2video,mpeg1video,mjpeg,prores,gif,rawvideo,wrapped_avframe
    --enable-parser=h264,hevc,vp8,vp9,mpeg4video,mpegvideo,mjpeg
    --enable-bsf=h264_mp4toannexb,hevc_mp4toannexb,vp9_superframe_split
    --enable-encoder=libx264,mjpeg,rawvideo
    --enable-muxer=mp4,mov,segment,image2,mjpeg,rawvideo,h264,null
    --enable-filter=scale,fps,format,split,reverse,concat,crop,pad,setsar,setpts,trim,null,select,color,testsrc,nullsrc
    --enable-indev=lavfi
    "${ffmpeg_flags[@]}"
)
(
    cd "${build_root}/ffmpeg-${FFMPEG_VERSION}"
    PKG_CONFIG_PATH="${prefix}/lib/pkgconfig" "${ffmpeg_configure[@]}"
    make -j"$jobs" "ffmpeg${exe_suffix}" "ffprobe${exe_suffix}"
)

for tool in ffmpeg ffprobe; do
    dest="${out_dir}/keyglass-${tool}${exe_suffix}"
    cp "${build_root}/ffmpeg-${FFMPEG_VERSION}/${tool}${exe_suffix}" "$dest"
    case "$target" in
        *-apple-darwin) strip -x "$dest" ;;
        *) strip "$dest" ;;
    esac
    chmod 0755 "$dest"
done
cp "${build_root}/ffmpeg-${FFMPEG_VERSION}/COPYING.GPLv2" "${out_dir}/keyglass-ffmpeg-COPYING.GPLv2.txt"

{
    echo "keyglass-ffmpeg and keyglass-ffprobe build for ${target}"
    echo
    echo "License: GPL-2.0-or-later (FFmpeg configured with --enable-gpl and libx264)"
    echo "FFmpeg ${FFMPEG_VERSION} (sha256 ${FFMPEG_SHA256})"
    echo "x264 ${X264_COMMIT}"
    echo
    echo "x264 configure:"
    printf '  %q' "${x264_configure[@]}"; echo
    echo
    echo "FFmpeg configure:"
    printf '  %q' "${ffmpeg_configure[@]}"; echo
    echo
    echo "Compiler:"
    ${cc%% *} --version 2>&1 | sed 's/^/  /'
} > "${out_dir}/keyglass-ffmpeg-${target}.buildinfo.txt"

"${out_dir}/keyglass-ffmpeg${exe_suffix}" -hide_banner -version | head -n 1 || true
"${out_dir}/keyglass-ffprobe${exe_suffix}" -hide_banner -version | head -n 1 || true
echo "build-ffmpeg: wrote keyglass-ffmpeg${exe_suffix} and keyglass-ffprobe${exe_suffix} to ${out_dir}"
